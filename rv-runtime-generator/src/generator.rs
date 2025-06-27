// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

use crate::crate_type::*;
use crate::linker::*;
use crate::rt::*;

pub struct RuntimeConfig<'a> {
    pub rt_dirpath_name: &'a str,
    pub linker_dirpath_name: &'a str,
    pub linker_config: LinkerConfig<'a>,
    pub rt_config: RtConfig,
}

pub fn write_rv_runtime_files_as_module<'a>(
    runtime_config: &'a RuntimeConfig<'a>,
) -> std::io::Result<()> {
    write_rv_runtime_files(runtime_config, CrateType::Module)
}

pub fn write_rv_runtime_files_as_library<'a>(
    runtime_config: &'a RuntimeConfig<'a>,
) -> std::io::Result<()> {
    write_rv_runtime_files(runtime_config, CrateType::Library)
}

pub fn write_rv_runtime_files<'a>(
    runtime_config: &'a RuntimeConfig<'a>,
    crate_type: CrateType,
) -> std::io::Result<()> {
    write_linker_files(
        runtime_config.linker_dirpath_name,
        &runtime_config.linker_config,
        crate_type,
    )?;
    write_rt_files(
        runtime_config.rt_dirpath_name,
        &runtime_config.rt_config,
        crate_type,
    )?;
    Ok(())
}
