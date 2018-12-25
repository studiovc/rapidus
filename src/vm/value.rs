#![macro_use]

use super::callobj::{CallObject, CallObjectRef};
use super::error::*;
use builtin::{BuiltinFuncInfo, BuiltinFuncTy, BuiltinJITFuncInfo};
use builtins::function;
use bytecode_gen::ByteCode;
use chrono::{DateTime, Utc};
pub use gc;
use gc::GcType;
use id::Id;
pub use rustc_hash::FxHashMap;
use std::ffi::CString;

pub type FuncId = Id;

pub type RawStringPtr = *mut libc::c_char;

pub type NVP = (String, Property);
pub type PropMap = GcType<FxHashMap<String, Property>>;

#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub val: Value,
    pub writable: bool,
    pub enumerable: bool,
    pub configurable: bool,
}

// Now 16 bytes
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Empty,
    Null,
    Undefined,
    Bool(bool),
    Number(f64),
    String(Box<CString>), // TODO: Using CString is good for JIT. However, we need better one instead.
    Function(Box<(FuncId, ByteCode, PropMap, CallObject)>),
    BuiltinFunction(Box<(BuiltinFuncInfo, PropMap, CallObject)>), // id(==0:unknown)
    Object(PropMap), // Object(FxHashMap<String, Value>),
    Array(GcType<ArrayValue>),
    Date(Box<(DateTime<Utc>, PropMap)>),
    Arguments, // TODO: Should have CallObject
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrayValue {
    pub elems: Vec<Property>,
    pub length: usize,
    pub obj: PropMap,
}

#[macro_export]
macro_rules! make_nvp {
    ($($property_name:ident : $val:expr),*) => {
        vec![ $((stringify!($property_name).to_string(), Property::new($val))),* ]
    };
}

#[macro_export]
macro_rules! make_object {
    ($($property_name:ident : $val:expr),*) => {
        Value::object_from_nvp(&make_nvp!($($property_name: $val),*))
    };
}

impl Property {
    pub fn new(val: Value) -> Property {
        Property {
            val: val,
            writable: true,
            enumerable: true,
            configurable: true,
        }
    }
}

impl Value {
    /// convert to Property.
    pub fn to_property(&self) -> Property {
        Property::new(self.clone())
    }

    pub fn empty() -> Value {
        Value::Empty
    }

    pub fn string(s: String) -> Value {
        Value::String(Box::new(CString::new(s).unwrap()))
    }

    /// make JS function object.
    pub fn function(id: FuncId, iseq: ByteCode, callobj: CallObject) -> Value {
        let prototype = Value::object_from_nvp(&vec![]);
        let val = Value::Function(Box::new((
            id,
            iseq,
            Value::propmap_from_nvp(&make_nvp!(
                prototype:  prototype.clone(),
                __proto__:  function::FUNCTION_PROTOTYPE.with(|x| x.clone())
            )),
            callobj,
        )));

        prototype.set_constructor(val.clone());

        val
    }

    pub fn builtin_function_with_jit(
        func: BuiltinFuncTy,
        builtin_jit_func_info: BuiltinJITFuncInfo,
    ) -> Value {
        Value::builtin_function(func, Some(builtin_jit_func_info), &mut vec![], None)
    }

    pub fn default_builtin_function(func: BuiltinFuncTy) -> Value {
        Value::builtin_function(func, None, &mut vec![], None)
    }

    pub fn builtin_function(
        func: BuiltinFuncTy,
        builtin_jit_func_info: Option<BuiltinJITFuncInfo>,
        nvp: &mut Vec<NVP>,
        prototype: Option<Value>,
    ) -> Value {
        if let Some(prototype) = prototype {
            nvp.push(("prototype".to_string(), Property::new(prototype)));
        }
        let map = Value::propmap_from_nvp(nvp);
        Value::BuiltinFunction(Box::new((
            BuiltinFuncInfo::new(func, builtin_jit_func_info),
            map,
            CallObject::new(Value::Undefined),
        )))
    }

    pub fn builtin_constructor_from_nvp(
        func: BuiltinFuncTy,
        nvp: &mut Vec<NVP>,
        prototype: Option<Value>,
    ) -> Value {
        Value::builtin_function(func, None, nvp, prototype)
    }

    /// make new property map (PropMap) from nvp.
    pub fn propmap_from_nvp(nvp: &Vec<NVP>) -> PropMap {
        let mut map = FxHashMap::default();
        for p in nvp {
            map.insert(p.0.clone(), p.1.clone());
        }
        gc::new(map)
    }

    /// register name-value pairs to property map.
    pub fn insert_propmap(map: PropMap, nvp: &Vec<(&'static str, Value)>) {
        unsafe {
            for p in nvp {
                (*map).insert(p.0.to_string(), p.1.to_property());
            }
        }
    }

    pub fn object(map: PropMap) -> Value {
        use builtins::object;
        unsafe {
            (*map)
                .entry("__proto__".to_string())
                .or_insert(object::OBJECT_PROTOTYPE.with(|x| x.clone()).to_property());
            Value::Object(map)
        }
    }

    /// make new object from nvp.
    pub fn object_from_nvp(nvp: &Vec<NVP>) -> Value {
        let map = Value::propmap_from_nvp(&nvp);
        Value::object(map)
    }

    pub fn array(map: PropMap) -> Value {
        let mut ary = ArrayValue::new(vec![]);
        ary.obj = map;
        Value::Array(gc::new(ary))
    }

    /// make new array from elements.
    pub fn array_from_elems(elms: Vec<Value>) -> Value {
        let ary = ArrayValue::new(elms);
        Value::Array(gc::new(ary))
    }

    pub fn date(time_val: DateTime<Utc>) -> Value {
        use builtins::date::DATE_PROTOTYPE;
        Value::Date(Box::new((time_val, {
            let mut hm = FxHashMap::default();
            hm.insert(
                "__proto__".to_string(),
                Property::new(DATE_PROTOTYPE.with(|x| x.clone())),
            );
            gc::new(hm)
        })))
    }

    pub fn arguments() -> Value {
        Value::Arguments
    }

    pub fn get_property(&self, property: Value, callobjref: Option<&CallObjectRef>) -> Value {
        let property_of_number = || -> Value {
            use builtins::number::NUMBER_PROTOTYPE;
            let val = NUMBER_PROTOTYPE.with(|x| x.clone());
            set_this(obj_find_val(val, property.to_string().as_str()), self)
        };

        let property_of_object = |obj: Value| -> Value {
            set_this(obj_find_val(obj, property.to_string().as_str()), self)
        };

        let property_of_string = |s: &CString| -> Value {
            match property {
                // Character at the index 'n'
                Value::Number(n) if is_integer(n) => Value::string(
                    s.to_str()
                        .unwrap()
                        .chars()
                        .nth(n as usize)
                        .unwrap()
                        .to_string(),
                ),
                // Length of string. TODO: Is this implementation correct?
                Value::String(ref member) if member.to_str().unwrap() == "length" => Value::Number(
                    s.to_str()
                        .unwrap()
                        .chars()
                        .fold(0, |x, c| x + c.len_utf16()) as f64,
                ),
                // TODO: Support all features.
                _ => Value::Undefined,
            }
        };

        let property_of_array = |obj: &Value| -> Value {
            let get_by_idx = |n: usize| -> Value {
                if let Value::Array(ref arrval) = obj {
                    unsafe {
                        let arr = &(**arrval).elems;
                        if n >= (**arrval).length {
                            return Value::Undefined;
                        }

                        match arr[n].val {
                            Value::Empty => Value::Undefined,
                            ref other => other.clone(),
                        }
                    }
                } else {
                    unreachable!("get_property(): Value is not an array.");
                }
            };

            match property {
                // Index
                Value::Number(n) if is_integer(n) && n >= 0.0 => get_by_idx(n as usize),
                Value::String(ref s) if s.to_str().unwrap() == "length" => {
                    if let Value::Array(ref arrval) = obj {
                        unsafe { Value::Number((**arrval).length as f64) }
                    } else {
                        unreachable!("get_property(): Value is not an array.");
                    }
                }
                Value::String(ref s) => {
                    // https://www.ecma-international.org/ecma-262/9.0/index.html#sec-array-exotic-objects
                    let num = property.to_uint32();
                    if Value::Number(num).to_string() == s.to_str().unwrap() {
                        get_by_idx(num as usize)
                    } else {
                        set_this(obj_find_val(obj.clone(), &property.to_string()), self)
                    }
                }
                _ => obj_find_val(obj.clone(), &property.to_string()),
            }
        };

        let property_of_arguments = || -> Value {
            {
                match property {
                    // Index
                    Value::Number(n) if is_integer(n) && n >= 0.0 => callobjref
                        .and_then(|co| unsafe {
                            Some((**co).get_arguments_nth_value(n as usize).unwrap())
                        })
                        .unwrap_or_else(|| Value::Undefined),
                    Value::String(ref s) if s.to_str().unwrap() == "length" => {
                        let length = callobjref
                            .and_then(|co| unsafe { Some((**co).get_arguments_length()) })
                            .unwrap_or(0);
                        Value::Number(length as f64)
                    }
                    _ => Value::Undefined,
                }
            }
        };

        match self {
            Value::Number(_) => property_of_number(),
            Value::String(ref s) => property_of_string(s),
            Value::BuiltinFunction(_) | Value::Function(_) | Value::Date(_) | Value::Object(_) => {
                property_of_object(self.clone())
            }
            Value::Array(_) => property_of_array(&*self),
            Value::Arguments => property_of_arguments(),
            // TODO: Implement
            _ => Value::Undefined,
        }
    }

    pub fn set_property(&self, property: Value, value: Value, callobjref: Option<&CallObjectRef>) {
        fn set_by_idx(ary: &mut ArrayValue, n: usize, val: Value) {
            if n >= ary.length as usize {
                ary.length = n + 1;
                while ary.elems.len() < n + 1 {
                    ary.elems.push(Value::empty().to_property());
                }
            }
            ary.elems[n] = val.to_property();
        };

        match self {
            Value::Object(map)
            | Value::Date(box (_, map))
            | Value::Function(box (_, _, map, _))
            | Value::BuiltinFunction(box (_, map, _)) => unsafe {
                let refval = (**map)
                    .entry(property.to_string())
                    .or_insert_with(|| Value::Undefined.to_property());
                *refval = value.to_property();
            },
            Value::Array(ref aryval) => {
                match property {
                    // Index
                    Value::Number(n) if is_integer(n) && n >= 0.0 => unsafe {
                        set_by_idx(&mut **aryval, n as usize, value)
                    },
                    Value::String(ref s) if s.to_str().unwrap() == "length" => match value {
                        Value::Number(n) if is_integer(n) && n >= 0.0 => unsafe {
                            (**aryval).length = n as usize;
                            while (**aryval).elems.len() < n as usize + 1 {
                                (**aryval).elems.push(Value::empty().to_property());
                            }
                        },
                        _ => {}
                    },
                    // https://www.ecma-international.org/ecma-262/9.0/index.html#sec-array-exotic-objects
                    Value::String(ref s)
                        if Value::Number(property.to_uint32()).to_string()
                            == s.to_str().unwrap() =>
                    {
                        let num = property.to_uint32();
                        unsafe { set_by_idx(&mut **aryval, num as usize, value) }
                    }
                    _ => unsafe {
                        let refval = (*(**aryval).obj)
                            .entry(property.to_string())
                            .or_insert_with(|| Value::Undefined.to_property());
                        *refval = value.to_property();
                    },
                }
            }
            Value::Arguments => {
                match property {
                    // Index
                    Value::Number(n) if n - n.floor() == 0.0 => unsafe {
                        (**callobjref.unwrap()).set_arguments_nth_value(n as usize, value);
                    },
                    // TODO: 'length'
                    _ => {}
                }
            }
            _ => {}
        };
    }

    pub fn set_number_if_possible(&mut self, n: f64) {
        if let Value::Number(ref mut n_) = self {
            *n_ = n;
        }
    }

    pub fn set_constructor(&self, constructor: Value) {
        match self {
            Value::Function(box (_, _, obj, _))
            | Value::BuiltinFunction(box (_, obj, _))
            | Value::Date(box (_, obj))
            | Value::Object(obj) => unsafe {
                (**obj).insert("constructor".to_string(), constructor.to_property());
            },
            Value::Array(aryval) => unsafe {
                (*((**aryval).obj)).insert("constructor".to_string(), constructor.to_property());
            },
            Value::Empty
            | Value::Null
            | Value::Undefined
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Arguments => {}
        }
    }
}

impl Value {
    pub fn to_string(&self) -> String {
        match self {
            Value::Undefined => "undefined".to_string(),
            Value::Bool(b) => {
                if *b {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            Value::Number(n) => {
                if n.is_nan() {
                    return "NaN".to_string();
                }

                if *n == 0.0 {
                    return "0".to_string();
                }

                if n.is_infinite() {
                    return "Infinity".to_string();
                }

                // TODO: Need a correct implementation!
                //  ref. https://tc39.github.io/ecma262/#sec-tostring-applied-to-the-number-type
                format!("{}", *n)
            }
            Value::String(s) => s.clone().into_string().unwrap(),
            Value::Array(ary_val) => unsafe { (**ary_val).to_string() },
            Value::Object(_) => "[object Object]".to_string(),
            Value::Date(box (time_val, _)) => time_val.to_rfc3339(),
            Value::Function(_) => "[Function]".to_string(),
            Value::BuiltinFunction(_) => "[BuiltinFunc]".to_string(),
            Value::Null => "null".to_string(),
            Value::Empty => "empty".to_string(),
            _ => "NOT IMPLEMENTED".to_string(),
        }
    }

    // TODO: Need a correct implementation!
    pub fn to_number(&self) -> f64 {
        fn str_to_num(s: &str) -> f64 {
            let all_ws = s.chars().all(|c| c.is_whitespace());

            if all_ws {
                return 0.0;
            }

            match s.parse::<f64>() {
                Ok(n) => n,
                _ => ::std::f64::NAN,
            }
        }

        fn ary_to_num(ary: &ArrayValue) -> f64 {
            match ary.length {
                0 => 0.0,
                // TODO: FIX!!!
                1 => match ary.elems[0].val {
                    Value::Bool(_) => ::std::f64::NAN,
                    ref otherwise => otherwise.to_number(),
                },
                _ => ::std::f64::NAN,
            }
        }

        match self {
            Value::Undefined => ::std::f64::NAN,
            Value::Bool(false) => 0.0,
            Value::Bool(true) => 1.0,
            Value::Number(n) => *n,
            Value::String(s) => str_to_num(s.to_str().unwrap()),
            Value::Array(ary) => ary_to_num(unsafe { &**ary }),
            _ => ::std::f64::NAN,
        }
    }

    pub fn to_uint32(&self) -> f64 {
        let num = self.to_number();
        let p2_32 = 4294967296i64;

        if num.is_nan() || num == 0.0 || num.is_infinite() {
            return 0.0;
        }

        let int32bit = (if num < 0.0 {
            -num.abs().floor()
        } else {
            num.abs().floor()
        } as i64
            % p2_32) as f64;

        if int32bit < 0.0 {
            p2_32 as f64 + int32bit
        } else {
            int32bit
        }
    }

    // TODO: Need a correct implementation!
    pub fn to_boolean(&self) -> bool {
        match self {
            Value::Undefined => false,
            Value::Bool(b) => *b,
            Value::Number(n) if *n == 0.0 || n.is_nan() => false,
            Value::Number(_) => true,
            Value::String(s) if s.to_str().unwrap().len() == 0 => false,
            Value::String(_) => true,
            Value::Array(_) => true,
            Value::Object(_) => true,
            _ => false,
        }
    }
}

impl Value {
    pub fn type_equal(&self, other: &Value) -> bool {
        match (self, other) {
            (&Value::Empty, Value::Empty)
            | (&Value::Null, Value::Null)
            | (&Value::Undefined, Value::Undefined)
            | (&Value::Bool(_), Value::Bool(_))
            | (&Value::Number(_), Value::Number(_))
            | (&Value::String(_), Value::String(_))
            | (&Value::Object(_), Value::Object(_))
            | (&Value::Function(_), Value::Function(_))
            | (&Value::BuiltinFunction(_), Value::BuiltinFunction(_))
            | (Value::Array(_), Value::Array(_))
            | (Value::Arguments, Value::Arguments) => true,
            _ => false,
        }
    }
    // https://tc39.github.io/ecma262/#sec-abstract-equality-comparison
    pub fn abstract_equal(self, other: Value) -> Result<bool, RuntimeError> {
        if self.type_equal(&other) {
            return self.strict_equal(other);
        }

        match (&self, &other) {
            (&Value::Number(l), &Value::String(_)) => Ok(l == other.to_number()),
            (&Value::String(_), &Value::Number(r)) => Ok(self.to_number() == r),
            (&Value::Bool(_), _) => Ok(Value::Number(self.to_number()).abstract_equal(other)?),
            (_, &Value::Bool(_)) => Ok(Value::Number(other.to_number()).abstract_equal(self)?),
            // TODO: Implement the following cases:
            //  8. If Type(x) is either String, Number, or Symbol and Type(y) is Object,
            //      return the result of the comparison x == ToPrimitive(y).
            //  9. If Type(x) is Object and Type(y) is either String, Number, or Symbol,
            //      return the result of the comparison ToPrimitive(x) == y.
            _ => Ok(false),
        }
    }

    // https://tc39.github.io/ecma262/#sec-strict-equality-comparison
    pub fn strict_equal(self, other: Value) -> Result<bool, RuntimeError> {
        match (self, other) {
            (Value::Empty, Value::Empty) => unreachable!(),
            (Value::Null, Value::Null) => Ok(true),
            (Value::Undefined, Value::Undefined) => Ok(true),
            (Value::Bool(l), Value::Bool(r)) => Ok(l == r),
            (Value::Number(l), Value::Number(r)) if l.is_nan() || r.is_nan() => Ok(false),
            (Value::Number(l), Value::Number(r)) => Ok(l == r),
            (Value::String(l), Value::String(r)) => Ok(l == r),
            (Value::Object(l), Value::Object(r)) => Ok(l == r),
            (Value::Function(box (l1, _, l2, _)), Value::Function(box (r1, _, r2, _))) => {
                Ok(l1 == r1 && l2 == r2)
            }
            (Value::BuiltinFunction(box (l1, l2, _)), Value::BuiltinFunction(box (r1, r2, _))) => {
                Ok(l1 == r1 && l2 == r2)
            }
            (Value::Array(l), Value::Array(r)) => Ok(l == r),
            (Value::Arguments, Value::Arguments) => return Err(RuntimeError::Unimplemented),
            _ => Ok(false),
        }
    }
}

impl ArrayValue {
    pub fn new(arr: Vec<Value>) -> ArrayValue {
        let len = arr.len();
        ArrayValue {
            elems: arr.iter().map(|x| x.to_property()).collect(),
            length: len,
            obj: {
                use builtins::array::ARRAY_PROTOTYPE;
                let nvp = make_nvp!(
                    __proto__:  ARRAY_PROTOTYPE.with(|x| x.clone())
                );
                Value::propmap_from_nvp(&nvp)
            },
        }
    }

    pub fn to_string(&self) -> String {
        self.elems[0..self.length]
            .iter()
            .fold("".to_string(), |acc, prop| {
                acc + prop.val.to_string().as_str() + ","
            })
            .trim_right_matches(",")
            .to_string()
    }

    pub fn push(&mut self, val: Value) {
        self.elems.push(Property::new(val));
        self.length += 1;
    }
}

// Utils

#[inline]
fn is_integer(f: f64) -> bool {
    f - f.floor() == 0.0
}

///
/// get <key> property of <val> object.
/// if the property does not exists, trace the prototype chain.
/// return Value::Undefined for primitives.
/// handle as BuiltinFunction.__proto__ === FUNCTION_PROTOTYPE
///
pub fn obj_find_val(val: Value, key: &str) -> Value {
    let (map, is_builtin_func) = match val {
        Value::BuiltinFunction(box (_, map, _)) => (map, true),
        Value::Function(box (_, _, map, _)) | Value::Date(box (_, map)) | Value::Object(map) => {
            (map, false)
        }
        Value::Array(aryval) => (unsafe { (*aryval).obj }, false),
        _ => return Value::Undefined,
    };
    unsafe {
        match (*map).get(key) {
            Some(prop) => prop.val.clone(),
            None if is_builtin_func && key == "__proto__" => {
                return function::FUNCTION_PROTOTYPE.with(|x| x.clone());
            }
            None => match (*map).get("__proto__") {
                Some(prop) => obj_find_val(prop.val.clone(), key),
                None if is_builtin_func => {
                    obj_find_val(function::FUNCTION_PROTOTYPE.with(|x| x.clone()), key)
                }
                _ => return Value::Undefined,
            },
        }
    }
}

///
/// if val is Function or BuiltinFunction, clone val and set this for callobj.this.
/// otherwise, do nothing.
///
pub fn set_this(val: Value, this: &Value) -> Value {
    match val.clone() {
        Value::Function(box (id, iseq, map, mut callobj)) => {
            Value::Function(Box::new((id, iseq, map, {
                *callobj.this = this.clone();
                callobj
            })))
        }
        Value::BuiltinFunction(box (id, obj, mut callobj)) => {
            Value::BuiltinFunction(Box::new((id, obj, {
                *callobj.this = this.clone();
                callobj
            })))
        }
        val => val,
    }
}
