// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

use lazy_static::lazy_static;
use std::collections::HashMap;

pub const START_SYMBOL: &str = "_start";

#[derive(Debug, Hash, Eq, PartialEq, Copy, Clone)]
pub enum GeneratedFunc {
    BootId,
    HartId,
    TpBlockAddr,
    TrapFrameAddr,
    TpBlockBase,
    TpBlockSlice,
    TpBlock,
    SwitchTo,
    RestoreTrapFrame,
}

pub struct GeneratedFuncMap {
    map: HashMap<GeneratedFunc, &'static str>,
}

impl GeneratedFuncMap {
    pub fn asm_fn(&self, func: GeneratedFunc) -> String {
        format!("__{:#}", self.map.get(&func).unwrap())
    }

    pub fn rust_fn(&self, func: GeneratedFunc) -> String {
        format!("{:#}", self.map.get(&func).unwrap())
    }
}

lazy_static! {
    pub static ref GEN_FUNC_MAP: GeneratedFuncMap = GeneratedFuncMap {
        map: [
            (GeneratedFunc::BootId, "my_boot_id"),
            (GeneratedFunc::HartId, "my_hart_id"),
            (GeneratedFunc::TpBlockAddr, "my_tpblock_addr"),
            (GeneratedFunc::TrapFrameAddr, "my_trap_frame_addr"),
            (GeneratedFunc::TpBlockBase, "tpblock_base"),
            (GeneratedFunc::TpBlockSlice, "tp_block_slice"),
            (GeneratedFunc::TpBlock, "my_tpblock_mut"),
            (GeneratedFunc::SwitchTo, "switch_to"),
            (GeneratedFunc::RestoreTrapFrame, "get_restore_tf_label"),
        ]
        .iter()
        .copied()
        .collect(),
    };
}
