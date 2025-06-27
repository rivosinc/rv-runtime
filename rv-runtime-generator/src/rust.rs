// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

use std::cell::RefCell;

use crate::file_writer::*;

#[derive(Debug)]
pub enum RustSentence {
    StructStart(String), // (struct name)
    StructEnd,
    StructField(String, String), // (field name, field type)
    MethodStart(String, bool, Option<String>, Option<String>), // (method name, is self mut, optional arg, optional ret)
    MethodEnd,
    ImplStart(String), // (impl name)
    ImplEnd,
    GetSelfMember(String),         // (self member name)
    SetSelfMember(String, String), // (self member name, param name)
    ExternStart(String),           // (ffi name)
    ExternEnd,
    StaticDef(String, String),                         // (name, type)
    FuncStart(String, Option<String>, Option<String>), // (function name, optional arg, optional ret)
    FuncEnd,
    AddrOf(String),                                     // (var)
    Use(String),                                        // (use name)
    Sub(String, String),                                // (var1, var2)
    FuncPrototype(String, Vec<String>, Option<String>), // (function name, args, optional ret)
    UnsafeStart,
    UnsafeEnd,
    CallWithRet(String, Vec<String>), // (called function name, args)
    CallWithoutRet(String, Vec<String>), // (called function name, args)
    ImplicitRet(String),              // (return value)
    ExplicitRet(String),              // (return value)
    ForIter(String, String),          // (element, iterator)
    ForEnd,
    IfEq(String, String), // (left, right)
    IfEnd,
    Comment(String),                                // // comment_string
    EnumStart(String, Vec<String>, Option<String>), // (enum name, custom derive, repr)
    EnumEnd,
    EnumCaseValue(String, usize), // (case name, value)
}

impl RustSentence {
    pub fn generate(&self, fw: &FileWriter) {
        match self {
            Self::StructStart(name) => {
                fw.add_line("#[repr(C)]");
                fw.add_line("#[derive(Debug, Copy, Clone)]");
                fw.new_block(&format!("pub struct {name:#}"));
            }
            Self::StructEnd
            | Self::MethodEnd
            | Self::ImplEnd
            | Self::ExternEnd
            | Self::FuncEnd
            | Self::UnsafeEnd
            | Self::ForEnd
            | Self::IfEnd
            | Self::EnumEnd => fw.end_block(),
            Self::StructField(name, ty) => fw.add_line(&format!("pub {name:#}: {ty:#},")),
            Self::MethodStart(name, mut_self, arg, ret) => {
                fw.add_line("#[allow(dead_code, non_snake_case)]");
                fw.new_block(&format!(
                    "pub fn {:#}(&{:#}self{:#}){:#}",
                    name,
                    if *mut_self { "mut " } else { "" },
                    if let Some(arg) = arg {
                        format!(", {arg:#}")
                    } else {
                        "".to_string()
                    },
                    if let Some(retval) = ret {
                        format!(" -> {retval:#}")
                    } else {
                        "".to_string()
                    }
                ));
            }
            Self::FuncStart(name, arg, ret) => {
                fw.add_line("#[allow(dead_code, non_snake_case)]");
                fw.new_block(&format!(
                    "pub fn {:#}({:#}){:#}",
                    name,
                    if let Some(arg) = arg {
                        format!("{arg:#}")
                    } else {
                        "".to_string()
                    },
                    if let Some(retval) = ret {
                        format!(" -> {retval:#}")
                    } else {
                        "".to_string()
                    }
                ));
            }
            Self::ImplStart(name) => fw.new_block(&format!("impl {name:#}")),
            Self::GetSelfMember(name) => fw.add_line(&format!("self.{name:#}")),
            Self::SetSelfMember(name, param) => fw.add_line(&format!("self.{name:#} = {param:#};")),
            Self::ExternStart(ffi) => fw.new_block(&format!("extern {ffi:?}")),
            Self::StaticDef(name, ty) => fw.add_line(&format!("static {name:#}: {ty:#};")),
            Self::AddrOf(var) => fw.add_line(&format!("(addr_of!({var:#})) as usize")),
            Self::Use(use_name) => fw.add_line(&format!("use {use_name:#};")),
            Self::Sub(var1, var2) => fw.add_line(&format!("{var1:#} - {var2:#}")),
            Self::FuncPrototype(name, args, ret) => {
                fw.add_line(&format!(
                    "fn {:#}({:#}){:#};",
                    name,
                    args.join(","),
                    if let Some(ret) = ret {
                        format!(" -> {ret:#}")
                    } else {
                        "".to_string()
                    }
                ));
            }
            Self::UnsafeStart => fw.new_block("unsafe"),
            Self::CallWithRet(fn_name, args) => {
                fw.add_line(&format!("{:#}({:#})", fn_name, args.join(",")))
            }
            Self::CallWithoutRet(fn_name, args) => {
                fw.add_line(&format!("{:#}({:#});", fn_name, args.join(",")))
            }
            Self::ImplicitRet(ret) => {
                fw.add_line(&format!("{ret:#}"));
            }
            Self::ExplicitRet(ret) => {
                fw.add_line(&format!("return {ret:#};"));
            }
            Self::ForIter(elem, iter) => {
                fw.new_block(&format!("for {elem:#} in {iter:#}"));
            }
            Self::IfEq(left, right) => {
                fw.new_block(&format!("if {left:#} == {right:#}"));
            }
            Self::Comment(comment) => fw.add_line(&format!("// {comment:#}")),
            Self::EnumStart(name, custom_derive, repr) => {
                if let Some(s) = repr {
                    fw.add_line(&format!("#[repr({s})]"));
                }
                fw.add_line(&format!(
                    "#[derive(Debug, Copy, Clone{})]",
                    if custom_derive.is_empty() {
                        "".to_string()
                    } else {
                        format!(", {}", custom_derive.join(", "))
                    }
                ));
                fw.add_line("#[allow(dead_code, non_snake_case)]");
                fw.new_block(&format!("pub enum {name:#}"));
            }
            Self::EnumCaseValue(name, value) => {
                fw.add_line(&format!("{name} = {value:#x?},"));
            }
        }
    }
}

#[derive(Debug)]
pub struct RustBuilder {
    sentences: RefCell<Vec<RustSentence>>,
}

impl RustBuilder {
    pub fn new() -> Self {
        let rb = Self {
            sentences: RefCell::new(Vec::new()),
        };

        rb.comment(&auto_generate_banner());
        rb
    }

    pub fn add_sentence(&self, sentence: RustSentence) {
        self.sentences.borrow_mut().push(sentence);
    }

    pub fn new_struct(&self, name: String) {
        self.add_sentence(RustSentence::StructStart(name));
    }

    pub fn new_struct_field(&self, field_name: String, field_type: String) {
        self.add_sentence(RustSentence::StructField(field_name, field_type));
    }

    pub fn end_struct(&self) {
        self.add_sentence(RustSentence::StructEnd);
    }

    pub fn generate(&self, fw: &FileWriter) {
        for sentence in self.sentences.borrow().iter() {
            sentence.generate(fw);
        }
    }

    pub fn new_method_with_ret(&self, name: String, ret: String) {
        self.add_sentence(RustSentence::MethodStart(name, false, None, Some(ret)));
    }

    pub fn new_method_self_mut_with_arg(&self, name: String, arg: String) {
        self.add_sentence(RustSentence::MethodStart(name, true, Some(arg), None));
    }

    pub fn new_method_self_mut(&self, name: String) {
        self.add_sentence(RustSentence::MethodStart(name, true, None, None));
    }

    pub fn end_method(&self) {
        self.add_sentence(RustSentence::MethodEnd);
    }

    pub fn new_func_with_ret(&self, name: String, ret: String) {
        self.add_sentence(RustSentence::FuncStart(name, None, Some(ret)));
    }

    pub fn new_func_with_arg_and_ret(&self, name: String, arg: String, ret: String) {
        self.add_sentence(RustSentence::FuncStart(name, Some(arg), Some(ret)));
    }

    pub fn new_func_with_arg(&self, name: String, arg: String) {
        self.add_sentence(RustSentence::FuncStart(name, Some(arg), None));
    }

    pub fn end_func(&self) {
        self.add_sentence(RustSentence::FuncEnd);
    }

    pub fn new_impl(&self, name: String) {
        self.add_sentence(RustSentence::ImplStart(name));
    }

    pub fn end_impl(&self) {
        self.add_sentence(RustSentence::ImplEnd);
    }

    pub fn get_self_member(&self, name: String) {
        self.add_sentence(RustSentence::GetSelfMember(name));
    }

    pub fn set_self_member(&self, name: String, param: String) {
        self.add_sentence(RustSentence::SetSelfMember(name, param));
    }

    pub fn new_c_extern(&self) {
        self.add_sentence(RustSentence::ExternStart("C".to_string()));
    }

    pub fn end_extern(&self) {
        self.add_sentence(RustSentence::ExternEnd);
    }

    pub fn static_def(&self, name: String, ty: String) {
        self.add_sentence(RustSentence::StaticDef(name, ty));
    }

    pub fn func_prototype(&self, fn_name: String, args: Vec<String>, ret: Option<String>) {
        self.add_sentence(RustSentence::FuncPrototype(fn_name, args, ret));
    }

    pub fn addr_of(&self, var: String) {
        self.add_sentence(RustSentence::AddrOf(var));
    }

    pub fn new_use(&self, use_name: String) {
        self.add_sentence(RustSentence::Use(use_name));
    }

    pub fn sub(&self, var1: String, var2: String) {
        self.add_sentence(RustSentence::Sub(var1, var2));
    }

    pub fn new_unsafe_block(&self) {
        self.add_sentence(RustSentence::UnsafeStart);
    }

    pub fn end_unsafe_block(&self) {
        self.add_sentence(RustSentence::UnsafeEnd);
    }

    pub fn call_with_ret(&self, fn_name: String, args: Vec<String>) {
        self.add_sentence(RustSentence::CallWithRet(fn_name, args));
    }

    pub fn call_without_ret(&self, fn_name: String, args: Vec<String>) {
        self.add_sentence(RustSentence::CallWithoutRet(fn_name, args));
    }

    pub fn implicit_ret(&self, ret: String) {
        self.add_sentence(RustSentence::ImplicitRet(ret));
    }

    pub fn explicit_ret(&self, ret: String) {
        self.add_sentence(RustSentence::ExplicitRet(ret));
    }

    pub fn for_iter(&self, elem: &str, iter: &str) {
        self.add_sentence(RustSentence::ForIter(elem.to_string(), iter.to_string()));
    }

    pub fn end_for(&self) {
        self.add_sentence(RustSentence::ForEnd);
    }

    pub fn if_eq(&self, left: &str, right: &str) {
        self.add_sentence(RustSentence::IfEq(left.to_string(), right.to_string()));
    }

    pub fn end_if(&self) {
        self.add_sentence(RustSentence::IfEnd);
    }

    pub fn comment(&self, comment: &str) {
        self.add_sentence(RustSentence::Comment(comment.to_string()));
    }

    pub fn new_enum<T: ToString, U: ToString>(&self, name: T, repr: Option<U>) {
        self.add_sentence(RustSentence::EnumStart(
            name.to_string(),
            Vec::new(),
            repr.map(|s| s.to_string()),
        ));
    }

    pub fn end_enum(&self) {
        self.add_sentence(RustSentence::EnumEnd);
    }

    pub fn enum_case_value<T: ToString>(&self, name: T, value: usize) {
        self.add_sentence(RustSentence::EnumCaseValue(name.to_string(), value));
    }
}
