// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RvMode {
    MMode,
    SMode,
}

impl std::fmt::Display for RvMode {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::MMode => "m",
            Self::SMode => "s",
        };
        write!(f, "{print_str}")
    }
}

impl RvMode {
    pub fn as_pp(&self) -> usize {
        match self {
            // MPP as M-mode
            Self::MMode => 3 << 11,
            // SPP as S-mode
            Self::SMode => 1 << 8,
        }
    }

    pub fn as_mask(&self) -> usize {
        // Values are the same
        self.as_pp()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RvXlen {
    Rv32,
    Rv64,
}

impl RvXlen {
    fn bytes(&self) -> isize {
        match self {
            Self::Rv32 => 4,
            Self::Rv64 => 8,
        }
    }

    fn word_prefix(&self) -> &str {
        match self {
            Self::Rv32 => "w",
            Self::Rv64 => "d",
        }
    }
}

#[derive(Clone, Debug)]
pub struct HartConfig {
    pub rv_mode: RvMode,
    pub rv_xlen: RvXlen,
    pub max_hart_count: usize,
    pub all_harts_start_at_reset_vector: bool,
}

impl HartConfig {
    pub fn new(
        rv_mode: RvMode,
        rv_xlen: RvXlen,
        max_hart_count: usize,
        all_harts_start_at_reset_vector: bool,
    ) -> Self {
        Self {
            rv_mode,
            rv_xlen,
            max_hart_count,
            all_harts_start_at_reset_vector,
        }
    }

    pub fn multihart_reset_handling_required(&self) -> bool {
        self.all_harts_start_at_reset_vector && self.max_hart_count > 1
    }
}

#[derive(Clone, Debug)]
pub struct MemConfig {
    pub per_hart_stack_size: usize,
    pub heap_size: usize,
}

impl MemConfig {
    pub fn new(per_hart_stack_size: usize, heap_size: usize) -> Self {
        Self {
            per_hart_stack_size,
            heap_size,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TargetConfig {
    pub mem_config: MemConfig,
    pub hart_config: HartConfig,
    pub custom_reset_config: bool,
}

impl TargetConfig {
    pub fn max_hart_count(&self) -> usize {
        self.hart_config.max_hart_count
    }

    pub fn per_hart_stack_size(&self) -> usize {
        self.mem_config.per_hart_stack_size
    }

    pub fn heap_size(&self) -> usize {
        self.mem_config.heap_size
    }

    pub fn rv_mode(&self) -> RvMode {
        self.hart_config.rv_mode
    }

    pub fn rv_xlen(&self) -> RvXlen {
        self.hart_config.rv_xlen
    }

    pub fn xlen_bytes(&self) -> isize {
        self.hart_config.rv_xlen.bytes()
    }

    pub fn xlen_word_prefix(&self) -> &str {
        self.hart_config.rv_xlen.word_prefix()
    }

    pub fn multihart_reset_handling_required(&self) -> bool {
        self.hart_config.multihart_reset_handling_required()
    }

    pub fn is_multi_hart(&self) -> bool {
        self.hart_config.max_hart_count > 1
    }

    pub fn needs_custom_reset(&self) -> bool {
        self.custom_reset_config
    }
}
