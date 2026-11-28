//! The sealed box, against the properties that make it worth having and the attacks it refuses.

use super::*;

fn keypair() -> ([u8; KEY], SecretKey) {
    let secret = generate_secret().expect("a key");
    (public_of(&secret), secret)
}

const HERE: Context = Context::Named {
    store: "user",
    name: "token",
};

/// The seven low-order points the audit probed with, in their canonical encodings.
fn low_order_points() -> Vec<[u8; KEY]> {
    [
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0100000000000000000000000000000000000000000000000000000000000000",
        "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
        "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    ]
    .iter()
    .map(|hex| {
        let mut point = [0u8; KEY];
        for (at, byte) in point.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).expect("hex");
        }
        point
    })
    .collect()
}

/// The count byte of an `OSLO3` file, and where its stanzas start.
const COUNT: usize = MAGIC.len() + 1 + KEY;
const STANZAS: usize = COUNT + 1;

/// An `OSLO3` file to `victim` whose writer is a low-order point, built with nobody's secret: the
/// writer's exchange is zero whichever scalar computes it.
fn forge_as_low_order_writer(victim: &[u8; KEY], point: &[u8; KEY], value: &[u8]) -> Vec<u8> {
    let ephemeral = StaticSecret::from([5u8; KEY]);
    let public = PublicKey::from(&ephemeral).to_bytes();
    let es = ephemeral.diffie_hellman(&PublicKey::from(*victim));
    let ss = StaticSecret::from([6u8; KEY]).diffie_hellman(&PublicKey::from(*point));
    assert!(!ss.was_contributory());
    let key = wrapping_key(&es, &ss, &public, victim, point);
    let context = HERE.bytes();
    let header = [b"OSLO3".as_slice(), point, &[1]].concat();
    let file_key = [8u8; KEY];
    let wrapped = cipher(&key)
        .encrypt(
            &XNonce::from([0u8; NONCE]),
            Payload {
                msg: &file_key,
                aad: &[&header[..], &public, &context].concat(),
            },
        )
        .expect("wrap");
    let head = [&header[..], &public, &wrapped].concat();
    let body = cipher(&file_key)
        .encrypt(
            &XNonce::from([2u8; NONCE]),
            Payload {
                msg: value,
                aad: &[&head[..], &context].concat(),
            },
        )
        .expect("body");
    [&head[..], &[2u8; NONCE], &body].concat()
}

#[test]
fn what_goes_in_comes_back_out() {
    let (public, secret) = keypair();
    for value in [b"".as_slice(), b"short", &[0u8; 100_000]] {
        let sealed = seal(&secret, &[public], &HERE, value).expect("seal");
        assert!(sealed.starts_with(b"OSLO3"));
        assert!(
            !sealed.windows(value.len().max(1)).any(|w| w == value) || value.is_empty(),
            "the value is in the file in the clear"
        );
        assert_eq!(
            *unseal(&secret, &[], &HERE, &sealed).expect("unseal"),
            value
        );
    }
}

/// Two people, two secrets neither has shared, one file both can open, each listing the other.
#[test]
fn several_recipients_can_each_open_it_with_their_own_key() {
    let (mine, my_secret) = keypair();
    let (theirs, their_secret) = keypair();
    let (_, stranger_secret) = keypair();

    let sealed = seal(&my_secret, &[mine, theirs], &HERE, b"the deploy token").expect("seal");
    assert_eq!(
        *unseal(&my_secret, &[], &HERE, &sealed).expect("mine"),
        b"the deploy token"
    );
    assert_eq!(
        *unseal(&their_secret, &[mine], &HERE, &sealed).expect("theirs"),
        b"the deploy token"
    );
    assert_eq!(
        unseal(&stranger_secret, &[mine], &HERE, &sealed),
        Err(Refused::NotOurs)
    );
}

/// Two writes of one value differ, so two backups do not say which secrets did not change.
#[test]
fn the_same_value_seals_differently_every_time() {
    let (public, secret) = keypair();
    let once = seal(&secret, &[public], &HERE, b"the same value").expect("seal");
    let twice = seal(&secret, &[public], &HERE, b"the same value").expect("seal");
    assert_ne!(once, twice);
    assert_eq!(
        *unseal(&secret, &[], &HERE, &once).expect("a"),
        *unseal(&secret, &[], &HERE, &twice).expect("b")
    );
}

/// One changed bit anywhere is refused, and a truncated file says so rather than panicking.
#[test]
fn a_changed_file_is_refused_wherever_it_was_changed() {
    let (public, secret) = keypair();
    let (other, _) = keypair();
    let sealed = seal(&secret, &[other, public], &HERE, b"value").expect("seal");

    for at in 0..sealed.len() {
        let mut bent = sealed.clone();
        bent[at] ^= 1;
        assert!(
            unseal(&secret, &[], &HERE, &bent).is_err(),
            "a change at {at} opened"
        );
    }
    // The body, and another recipient's wrapped key, are both caught by the body's check.
    let mut body = sealed.clone();
    *body.last_mut().expect("a byte") ^= 1;
    assert_eq!(unseal(&secret, &[], &HERE, &body), Err(Refused::Tampered));
    let mut theirs = sealed.clone();
    theirs[STANZAS + KEY] ^= 1;
    assert_eq!(unseal(&secret, &[], &HERE, &theirs), Err(Refused::Tampered));

    for short in [0, 3, 5, COUNT, STANZAS + 10, sealed.len() - TAG - 1] {
        let refused = unseal(&secret, &[], &HERE, &sealed[..short]).expect_err("opened");
        assert!(
            matches!(refused, Refused::Format(_)),
            "{short}: {refused:?}"
        );
    }
    let refused = unseal(&secret, &[], &HERE, b"nonsense").expect_err("opened");
    assert_eq!(refused, Refused::Format("no OSLO header".to_string()));
    let mut future = sealed.clone();
    future[MAGIC.len()] = b'9';
    assert!(matches!(
        unseal(&secret, &[], &HERE, &future),
        Err(Refused::Format(_))
    ));
}

/// **A swapped, renamed or moved file does not open.** `cp prod.sealed test.sealed` was the probe.
#[test]
fn a_file_opens_only_under_the_name_and_store_it_was_sealed_for() {
    let (public, secret) = keypair();
    let prod = Context::Named {
        store: "user",
        name: "prod-token",
    };
    let sealed = seal(&secret, &[public], &prod, b"the prod value").expect("seal");

    for elsewhere in [
        Context::Named {
            store: "user",
            name: "test-token",
        },
        Context::Named {
            store: "work",
            name: "prod-token",
        },
        Context::Named {
            store: "userp",
            name: "rod-token",
        },
        Context::Anonymous,
    ] {
        assert_eq!(
            unseal(&secret, &[], &elsewhere, &sealed),
            Err(Refused::NotOurs),
            "{elsewhere:?} opened it"
        );
    }
    // And a seal with no name is not a secret anybody can plant under one.
    let anonymous = seal(&secret, &[public], &Context::Anonymous, b"lua").expect("seal");
    assert_eq!(
        unseal(&secret, &[], &prod, &anonymous),
        Err(Refused::NotOurs)
    );
    assert_eq!(
        *unseal(&secret, &[], &Context::Anonymous, &anonymous).expect("lua"),
        b"lua"
    );
}

/// **A stripped or added stanza, or a changed count, does not open.** Stripping was the probe.
#[test]
fn the_recipient_list_cannot_be_edited() {
    let (mine, my_secret) = keypair();
    let (theirs, _) = keypair();
    let sealed = seal(&my_secret, &[theirs, mine], &HERE, b"value").expect("seal");

    let mut stripped = sealed[..STANZAS].to_vec();
    stripped[COUNT] = 1;
    stripped.extend_from_slice(&sealed[STANZAS + STANZA..]);
    assert_eq!(
        unseal(&my_secret, &[], &HERE, &stripped),
        Err(Refused::NotOurs)
    );

    let mut added = sealed[..STANZAS].to_vec();
    added[COUNT] = 3;
    added.extend_from_slice(&sealed[STANZAS..STANZAS + STANZA]);
    added.extend_from_slice(&sealed[STANZAS..]);
    assert!(
        unseal(&my_secret, &[], &HERE, &added).is_err(),
        "an added stanza opened"
    );

    let mut recounted = sealed.clone();
    recounted[COUNT] = 1;
    assert!(
        unseal(&my_secret, &[], &HERE, &recounted).is_err(),
        "a changed count opened"
    );
}

/// **A published half is not enough to write a file that opens.** The audit's forge probe sealed
/// with nothing but the recipient's public half; now the writer has to be somebody the reader lists.
#[test]
fn a_file_forged_from_a_public_half_is_refused() {
    let (victim, victim_secret) = keypair();
    let (friend, friend_secret) = keypair();
    let (_, attacker_secret) = keypair();

    // The attacker writes as themselves: it opens, and the writer is not on the list.
    let forged = seal(&attacker_secret, &[victim], &HERE, b"evil").expect("seal");
    let attacker = public_of(&attacker_secret);
    assert_eq!(
        unseal(&victim_secret, &[friend], &HERE, &forged),
        Err(Refused::Writer(attacker))
    );

    // Claiming to be a listed writer needs that writer's secret for the second exchange.
    let mut claimed = forged.clone();
    claimed[MAGIC.len() + 1..COUNT].copy_from_slice(&friend);
    assert_eq!(
        unseal(&victim_secret, &[friend], &HERE, &claimed),
        Err(Refused::NotOurs)
    );

    // A low-order writer makes the second exchange zero, which anybody can compute: a file built
    // on it is refused even by a reader whose list somehow holds that point.
    for point in low_order_points() {
        let low = forge_as_low_order_writer(&victim, &point, b"evil");
        assert_eq!(
            unseal(&victim_secret, &[friend, point], &HERE, &low),
            Err(Refused::Writer(point))
        );
    }

    // The real friend, listed, is accepted; listing nobody accepts only the reader's own key.
    let genuine = seal(&friend_secret, &[victim], &HERE, b"good").expect("seal");
    assert_eq!(
        *unseal(&victim_secret, &[friend], &HERE, &genuine).expect("listed"),
        b"good"
    );
    assert_eq!(
        unseal(&victim_secret, &[], &HERE, &genuine),
        Err(Refused::Writer(friend))
    );
    let own = seal(&victim_secret, &[victim], &HERE, b"mine").expect("seal");
    assert_eq!(
        *unseal(&victim_secret, &[], &HERE, &own).expect("own"),
        b"mine"
    );
}

/// **A low-order point is refused wherever a public key comes in.** `OSLO-PUB-1:AAAA…=` was the
/// probe: accepted as a recipient, it made every later write readable by anybody.
#[test]
fn a_low_order_point_is_never_a_recipient() {
    let (public, secret) = keypair();
    assert!(!low_order(&public));
    for point in low_order_points() {
        assert!(low_order(&point), "{point:02x?}");
        let mut high = point;
        high[KEY - 1] |= 0x80;
        assert!(low_order(&high), "{high:02x?}");
        for text in [write_public(&point), write_public(&high)] {
            let refused = read_public(&text).expect_err("a low-order recipient was read");
            assert!(refused.contains("low-order"), "{refused}");
        }
        let refused = seal(&secret, &[public, point], &HERE, b"v").expect_err("sealed to it");
        assert!(refused.contains("low-order"), "{refused}");
    }
    assert!(read_public("OSLO-PUB-1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").is_err());
}

/// **A stanza whose exchange comes out as zero is skipped**: its wrapping key is one anybody can
/// compute, so opening it would hand over whatever the forger chose.
#[test]
fn a_stanza_with_a_low_order_ephemeral_is_skipped() {
    let (public, secret) = keypair();
    for point in low_order_points() {
        let mut salt = [0u8; KEY * 2];
        salt[..KEY].copy_from_slice(&point);
        salt[KEY..].copy_from_slice(&public);
        let mut key = [0u8; KEY];
        hkdf::Hkdf::<sha2::Sha256>::new(Some(&salt), &[0u8; KEY])
            .expand(b"oslo secret v2 wrap", &mut key)
            .expect("expand");
        let file_key = [4u8; KEY];
        let wrapped = cipher(&key)
            .encrypt(&XNonce::from([0u8; NONCE]), &file_key[..])
            .expect("wrap");
        let body = cipher(&file_key)
            .encrypt(&XNonce::from([1u8; NONCE]), b"forged".as_slice())
            .expect("body");
        let file = [
            b"OSLO2".as_slice(),
            &[1],
            &point,
            &wrapped,
            &[1u8; NONCE],
            &body,
        ]
        .concat();
        assert_eq!(unseal(&secret, &[], &HERE, &file), Err(Refused::NotOurs));
    }
}

/// `OSLO2` still opens, and says it is `OSLO2`, so a store can read what it already has.
#[test]
fn an_oslo2_file_is_still_read() {
    let (public, secret) = keypair();
    let old = legacy::seal(&public, b"from before");
    let parsed = parse(&old).expect("parse");
    assert!(parsed.legacy());
    assert_eq!(
        *open(&secret, &[], &HERE, &parsed).expect("open"),
        b"from before"
    );
    let (_, stranger) = keypair();
    assert_eq!(unseal(&stranger, &[], &HERE, &old), Err(Refused::NotOurs));
    let mut bent = old.clone();
    *bent.last_mut().expect("a byte") ^= 1;
    assert_eq!(unseal(&secret, &[], &HERE, &bent), Err(Refused::Tampered));
    assert!(
        !parse(&seal(&secret, &[public], &HERE, b"v").expect("seal"))
            .expect("parse")
            .legacy()
    );
}

/// Every refusal names what went wrong without repeating the value or a secret.
#[test]
fn a_refusal_never_carries_a_secret() {
    let (public, secret) = keypair();
    let (_, attacker) = keypair();
    let forged = seal(&attacker, &[public], &HERE, b"hunter2").expect("seal");
    let refused = unseal(&secret, &[], &HERE, &forged).expect_err("opened");
    let said = refused.to_string();
    assert!(said.contains(PUBLIC), "{said}");
    assert!(!said.contains("hunter2"));
    assert!(!said.contains(write_secret(&secret).trim()), "{said}");
    assert!(!said.contains(write_secret(&attacker).trim()), "{said}");
}

/// A secret and a recipient are written differently, so neither can be pasted for the other.
#[test]
fn a_key_and_a_recipient_do_not_look_alike() {
    let (public, secret) = keypair();
    let written = write_secret(&secret);
    let published = write_public(&public);

    assert!(written.starts_with("OSLO-KEY-1:"));
    assert!(published.starts_with("OSLO-PUB-1:"));
    assert_eq!(*read_secret(&written).expect("read"), *secret);
    assert_eq!(read_public(&published).expect("read"), public);

    assert!(
        read_secret(&published).is_err(),
        "a recipient was read as a key"
    );
    assert!(
        read_public(&written).is_err(),
        "a key was read as a recipient"
    );

    assert_eq!(
        *read_secret(&format!("# a comment\n{}", written.as_str())).expect("read"),
        *secret
    );
    for bad in [
        "",
        "# only a comment",
        "not-a-key",
        "OSLO-KEY-1:!!!",
        "OSLO-KEY-1:c2hvcnQ=",
    ] {
        assert!(read_secret(bad).is_err(), "{bad:?} was accepted");
    }
    assert_eq!(read_secret("").expect_err("empty"), "there is no key in it");
}

/// Writing to nobody is refused rather than producing a file nothing can open.
#[test]
fn a_store_with_no_recipients_cannot_be_written() {
    let (_, secret) = keypair();
    assert!(
        seal(&secret, &[], &HERE, b"value")
            .unwrap_err()
            .contains("no recipient")
    );
}
