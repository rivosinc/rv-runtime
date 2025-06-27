// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

use crate::file_writer::*;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub enum CrateType {
    Module,
    Library,
}

impl CrateType {
    fn filename(&self) -> &str {
        match self {
            Self::Module => "mod.rs",
            Self::Library => "lib.rs",
        }
    }

    fn is_library(&self) -> bool {
        match self {
            Self::Module => false,
            Self::Library => true,
        }
    }
}

pub fn create_root_rs_filewriter(dirpath: &Path, crate_type: CrateType) -> FileWriter {
    let filepath = dirpath.join(crate_type.filename());
    let fw = FileWriter::new(filepath, BlockDelimiter::Parens);

    fw.add_line(&format!("// {}", auto_generate_banner()));
    if crate_type.is_library() {
        // In case of module, no_std is expected to be added to the real crate root
        fw.add_line("#![no_std]");
        fw.add_line("#![allow(unused_imports)]");
    }

    fw
}

pub fn add_module(fw: &FileWriter, filepath: &Path) {
    let mod_name = filepath.file_stem().unwrap().to_str().unwrap();
    fw.add_line(&format!("mod {mod_name:#};"));
    fw.add_line(&format!("pub use {mod_name:#}::*;"));
}
