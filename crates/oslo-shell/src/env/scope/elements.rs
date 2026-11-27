//! Arrays as the rest of the shell reads and writes them: whole, or one element at a time.
//!
//! Every writer follows a nameref first and refuses a read-only name. An element written to a name
//! that holds a scalar turns the scalar into element 0 of a new array, which is what it already
//! was as far as `${b[0]}` was concerned.

use super::special::ArrayWrite;
use super::value::{Attrs, Value, Var};
use super::{AssocArray, Environment, ShellArray};

impl Environment {
    /// The indexed array `name` resolves to, if it is one.
    pub fn get_array(&self, name: &str) -> Option<&ShellArray> {
        let target = self.ref_target_name(name);
        self.vars.get(&*target)?.indexed()
    }

    /// The associative array `name` resolves to, if it is one.
    pub fn get_assoc(&self, name: &str) -> Option<&AssocArray> {
        let target = self.ref_target_name(name);
        self.vars.get(&*target)?.assoc()
    }

    pub fn is_assoc(&self, name: &str) -> bool {
        self.get_assoc(name).is_some()
    }

    /// Refuse a write to a read-only `name`, saying so. `true` means the write may go ahead.
    fn writable(&self, name: &str) -> bool {
        if self.vars.get(name).is_some_and(Var::readonly) {
            self.complain_readonly(name, "");
            return false;
        }
        true
    }

    /// Replace `name`'s value with `array`. `false` if the variable is read-only.
    ///
    /// How another subsystem publishes a computed array: `PIPESTATUS`, `BASH_REMATCH`, `read -a`.
    pub fn set_array(&mut self, name: &str, array: ShellArray) -> bool {
        let name = self.ref_target_name(name).into_owned();
        if !self.writable(&name) {
            return false;
        }
        if let Some(done) = self.special_array_write(&name, ArrayWrite::Whole(&array)) {
            return done;
        }
        self.store_value(&name, Value::Indexed(array))
    }

    /// Replace `name`'s value with an associative array.
    pub fn set_assoc(&mut self, name: &str, map: AssocArray) -> bool {
        let name = self.ref_target_name(name).into_owned();
        self.writable(&name) && self.store_value(&name, Value::Assoc(map))
    }

    /// Store a whole value, keeping the binding's attributes.
    fn store_value(&mut self, name: &str, value: Value) -> bool {
        let was_exported = self.chain_exports(name);
        let export = self.allexport();
        match self.vars.get_mut(name) {
            Some(var) => {
                var.value = value;
                var.attrs.remove(Attrs::UNSET);
            }
            None => {
                let attrs = match export {
                    true => Attrs::EXPORT,
                    false => Attrs::empty(),
                };
                self.vars.insert(name.to_string(), Var { value, attrs });
            }
        }
        if was_exported {
            self.sync_environ(name);
        }
        true
    }

    /// The binding an element write lands in, turned into an indexed array if it is a scalar.
    fn element_slot(&mut self, name: &str) -> &mut Var {
        let var = self
            .vars
            .entry(name.to_string())
            .or_insert_with(|| Var::unset(Attrs::empty()));
        if let Value::Scalar(text) = &var.value {
            let mut array = ShellArray::default();
            if var.is_set() {
                array.set(0, text.clone());
            }
            var.value = Value::Indexed(array);
        }
        var.attrs.remove(Attrs::UNSET);
        var
    }

    /// Set one element of an indexed array, through the name's attributes.
    pub fn set_array_element(&mut self, name: &str, index: i64, value: &str) -> bool {
        let name = self.ref_target_name(name).into_owned();
        if !self.writable(&name) {
            return false;
        }
        if let Some(done) = self.special_array_write(&name, ArrayWrite::Element(index, value)) {
            return done;
        }
        let value = self.transform(&name, value);
        let was_exported = self.chain_exports(&name);
        match &mut self.element_slot(&name).value {
            Value::Indexed(array) => array.set(index, value),
            Value::Assoc(map) => map.set(&index.to_string(), value),
            Value::Scalar(_) => {}
        }
        if was_exported {
            self.sync_environ(&name);
        }
        true
    }

    /// Set one entry of an associative array, through the name's attributes.
    pub fn set_assoc_element(&mut self, name: &str, key: &str, value: &str) -> bool {
        let name = self.ref_target_name(name).into_owned();
        if !self.writable(&name) {
            return false;
        }
        let value = self.transform(&name, value);
        let var = self
            .vars
            .entry(name.clone())
            .or_insert_with(|| Var::unset(Attrs::empty()));
        if !matches!(var.value, Value::Assoc(_)) {
            var.value = Value::Assoc(AssocArray::default());
        }
        var.attrs.remove(Attrs::UNSET);
        if let Value::Assoc(map) = &mut var.value {
            map.set(key, value);
        }
        true
    }

    /// Append one element after the highest index in use.
    pub fn append_array_element(&mut self, name: &str, value: &str) -> bool {
        let name = self.ref_target_name(name).into_owned();
        if !self.writable(&name) {
            return false;
        }
        if let Some(done) = self.special_array_write(&name, ArrayWrite::Append) {
            return done;
        }
        let value = self.transform(&name, value);
        if let Value::Indexed(array) = &mut self.element_slot(&name).value {
            array.push(value);
        }
        true
    }

    /// Make `name` an indexed array without disturbing a value it already has. `declare -a name`.
    pub fn declare_array(&mut self, name: &str) {
        let var = self
            .vars
            .entry(name.to_string())
            .or_insert_with(|| Var::unset(Attrs::empty()));
        if let Value::Scalar(text) = &var.value {
            let mut array = ShellArray::default();
            if var.is_set() {
                array.set(0, text.clone());
            }
            var.value = Value::Indexed(array);
        }
    }

    /// Make `name` an associative array without disturbing one it already is. `declare -A name`.
    ///
    /// A scalar's value becomes the entry `0`, as bash converts it.
    pub fn declare_assoc(&mut self, name: &str) {
        let var = self
            .vars
            .entry(name.to_string())
            .or_insert_with(|| Var::unset(Attrs::empty()));
        if let Value::Scalar(text) = &var.value {
            let mut map = AssocArray::default();
            if var.is_set() {
                map.set("0", text.clone());
            }
            var.value = Value::Assoc(map);
        }
    }

    /// Drop one element. `unset 'a[1]'`.
    pub fn unset_array_element(&mut self, name: &str, index: i64) {
        let name = self.ref_target_name(name).into_owned();
        if self.special_array_write(&name, ArrayWrite::Unset).is_some() {
            return;
        }
        match self.vars.get_mut(&name).map(|v| &mut v.value) {
            Some(Value::Indexed(array)) => array.remove(index),
            Some(Value::Scalar(_)) if index == 0 || index == -1 => self.unset_binding(&name),
            _ => {}
        }
    }

    /// Drop one entry. `unset 'm[key]'`.
    pub fn unset_assoc_element(&mut self, name: &str, key: &str) {
        let name = self.ref_target_name(name).into_owned();
        if let Some(Value::Assoc(map)) = self.vars.get_mut(&name).map(|v| &mut v.value) {
            map.remove(key);
        }
    }

    /// `unset r` where the nameref `r` names `a[sub]`.
    pub(super) fn unset_element_text(&mut self, name: &str, subscript: &str) {
        if self.is_assoc(name) {
            self.unset_assoc_element(name, subscript);
        } else if let Ok(index) = subscript.trim().parse::<i64>() {
            self.unset_array_element(name, index);
        }
    }

    /// Every indexed array name.
    pub fn array_names(&self) -> impl Iterator<Item = &str> {
        self.vars
            .iter()
            .filter(|(_, var)| var.indexed().is_some())
            .map(|(name, _)| name.as_str())
    }

    /// Publish an array the shell maintains, bypassing the read-only check.
    pub(super) fn publish_array(&mut self, name: &str, array: ShellArray) {
        match self.vars.get_mut(name) {
            Some(var) => {
                var.value = Value::Indexed(array);
                var.attrs.remove(Attrs::UNSET);
            }
            None => {
                self.vars.insert(
                    name.to_string(),
                    Var {
                        value: Value::Indexed(array),
                        attrs: Attrs::empty(),
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::env::Environment;
    use crate::env::scope::ShellArray;

    #[test]
    fn a_scalar_and_an_array_share_one_name() {
        let mut env = Environment::new();
        env.set_var("oslo_a1", "scalar", false);
        env.set_array("oslo_a1", ShellArray::from_values(["1", "2", "3"]));
        assert_eq!(env.get_var("oslo_a1"), Some("1"));
        env.set_var("oslo_a1", "4", false);
        assert_eq!(env.get_array("oslo_a1").unwrap().joined(" "), "4 2 3");
    }

    #[test]
    fn an_element_assignment_promotes_an_existing_scalar() {
        let mut env = Environment::new();
        env.set_var("oslo_a2", "hello", false);
        env.set_array_element("oslo_a2", 2, "world");
        let a = env.get_array("oslo_a2").unwrap();
        assert_eq!(a.joined(" "), "hello world");
        assert_eq!(a.indices().collect::<Vec<_>>(), vec![0, 2]);
    }

    #[test]
    fn unsetting_a_variable_removes_its_array_too() {
        let mut env = Environment::new();
        env.set_array("oslo_a3", ShellArray::from_values(["x"]));
        env.unset_var("oslo_a3");
        assert!(env.get_array("oslo_a3").is_none());
        assert_eq!(env.get_var("oslo_a3"), None);
    }

    /// `$m` on an associative array is the entry `0`, as bash reads it.
    #[test]
    fn an_assoc_reads_its_zero_entry_as_a_scalar() {
        let mut env = Environment::new();
        env.declare_assoc("oslo_m1");
        env.set_assoc_element("oslo_m1", "k", "v");
        assert_eq!(env.get_var("oslo_m1"), None);
        env.set_var("oslo_m1", "z", false);
        assert_eq!(env.get_assoc("oslo_m1").unwrap().get("0"), Some("z"));
    }

    #[test]
    fn the_pipeline_statuses_are_published_as_an_array() {
        let mut env = Environment::new();
        env.set_pipeline_status(vec![1, 0, 3]);
        let statuses = env.get_array("PIPESTATUS").expect("published");
        assert_eq!(statuses.joined(" "), "1 0 3");
        assert_eq!(env.pipeline_status(), &[1, 0, 3]);
    }
}
