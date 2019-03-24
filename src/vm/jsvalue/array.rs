// use super::super::frame::LexicalEnvironmentRef;
use super::value::*;
// use builtin::BuiltinFuncTy2;
// use bytecode_gen::ByteCode;

#[derive(Clone, Debug)]
pub struct ArrayObjectInfo {
    pub elems: Vec<Property2>,
}

impl ArrayObjectInfo {
    pub fn get_element(&self, idx: usize) -> Property2 {
        if idx >= self.elems.len() {
            return Property2::new_data_simple(Value2::undefined());
        }

        if let Property2::Data(DataProperty {
            val,
            writable,
            enumerable,
            configurable,
        }) = self.elems[idx]
        {
            return Property2::Data(DataProperty {
                val: val.to_undefined_if_empty(),
                writable,
                enumerable,
                configurable,
            });
        }

        self.elems[idx]
    }

    pub fn set_element(&mut self, idx: usize, val_: Value2) -> Option<Value2> {
        // Extend
        if idx >= self.elems.len() {
            self.set_length(idx + 1);
        }

        match self.elems[idx] {
            Property2::Data(DataProperty { ref mut val, .. }) => {
                *val = val_;
                None
            }
            Property2::Accessor(AccessorProperty { set, .. }) => {
                if set.is_undefined() {
                    None
                } else {
                    Some(set)
                }
            }
        }
    }

    pub fn set_length(&mut self, len: usize) {
        // Extend
        if self.elems.len() < len {
            while self.elems.len() < len {
                self.elems.push(Property2::new_data_simple(Value2::empty()))
            }
            return;
        }

        // Shorten
        if self.elems.len() > len {
            unsafe { self.elems.set_len(len) };
            return;
        }
    }
}
