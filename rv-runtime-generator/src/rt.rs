// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::crate_type::*;
use crate::file_writer::*;
use crate::func::*;
use crate::linker::*;
use crate::rust::*;
use crate::target_config::*;

const RV_INSTRUCTION_ALIGNMENT_BYTES: usize = 4;
const SENTRY_VALUE_RV64: usize = 0x2d5952544e45532d;
const SENTRY_VALUE_RV32: u32 = 0x4e45532d;

const STATUS_FS_MASK_DIRTY: usize = 3 << 13;
const STATUS_FS_CLEAN: usize = 2 << 13;

#[derive(Debug, Copy, Clone)]
#[repr(u8)]
// Each enum variant represents a bit in rt_flags. Since we aim to
// support both rv32 and rv64, we can have bits going from 0 to 31
// only.
pub enum RtFlagBit {
    // This flag is set for the conditions where we want to ensure
    // that the previous trapframe value must be restored in tpblock
    // when popping current trapframe from stack. This can be utilized
    // in the following conditions:
    // 1. Recursive trap handling - in this case, we want to ensure
    //    that the current trapframe value in tpblock is valid on return
    //    from the recursive trap. So, when the trap handling path
    //    encounters a recursive trap, it can set this flag to indicate
    //    that we are in a recursive trap handler and so the trapframe
    //    address in tpblock needs to be restored on return path.
    // 2. Context switch - in case of context switch, we can set this
    //    flag to indicate that a different context is being switched to
    //    and so when we return to the context being switched out, we want
    //    to ensure that the current trapframe address gets correctly
    //    restored.
    RestoreTrapFrameInTpBlock = 0,
    FsStateWasDirty = 1,
    // Indicates if switching to/from this trapframe results in address
    // translation/protection control registers being changed, thereby
    // requiring an sfence.vma to invalidate caches.
    TranslationRegChanged = 2,
    // This is to ensure that we support both rv32 and rv64 using a single
    // rt_flags field. For now, I don't think we would need more than 32
    // bits to track state.
    MaxFlagBit = 31,
}

impl RtFlagBit {
    fn as_mask(&self) -> isize {
        assert!(*self as u8 <= Self::MaxFlagBit as u8);
        1 << *self as u8
    }

    fn generate(rust: &RustBuilder) {
        rust.new_enum("RtFlags", Some("u32"));
        rust.enum_case_value(
            "RestoreTrapFrameInTpBlock",
            Self::RestoreTrapFrameInTpBlock.as_mask() as usize,
        );
        rust.enum_case_value("FsStateWasDirty", Self::FsStateWasDirty.as_mask() as usize);
        rust.enum_case_value(
            "TranslationRegChanged",
            Self::TranslationRegChanged.as_mask() as usize,
        );
        rust.end_enum();
    }
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub enum EntrypointType {
    BootHart,
    NonBootHart,
    Trap,
    CustomReset,
    StackOverflow,
}

#[derive(Debug)]
pub struct RtConfig {
    entrypoints: HashMap<EntrypointType, String>,
    trap_frame: TrapFrame,
    tp_block: TpBlock,
    thread_ctx: ThreadContext,
    target_config: TargetConfig,
    skip_bss_clearing: bool,
    stack_overflow_detection: bool,
    supports_atomic_extension: bool,
    floating_point_support: bool,
    sfence_on_trapframe_restore_feature: bool,
}

impl RtConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        entrypoints: HashMap<EntrypointType, String>,
        trap_frame: TrapFrame,
        tp_block: TpBlock,
        thread_ctx: ThreadContext,
        target_config: TargetConfig,
        skip_bss_clearing: bool,
        stack_overflow_detection: bool,
        supports_atomic_extension: bool,
        floating_point_support: bool,
        sfence_on_trapframe_restore_feature: bool,
    ) -> Self {
        let mut s = Self {
            entrypoints,
            trap_frame,
            tp_block,
            thread_ctx,
            target_config,
            skip_bss_clearing,
            stack_overflow_detection,
            supports_atomic_extension,
            floating_point_support,
            sfence_on_trapframe_restore_feature,
        };

        if floating_point_support {
            for fr in [
                FloatingPointRegister::F0,
                FloatingPointRegister::F1,
                FloatingPointRegister::F2,
                FloatingPointRegister::F3,
                FloatingPointRegister::F4,
                FloatingPointRegister::F5,
                FloatingPointRegister::F6,
                FloatingPointRegister::F7,
                FloatingPointRegister::F8,
                FloatingPointRegister::F9,
                FloatingPointRegister::F10,
                FloatingPointRegister::F11,
                FloatingPointRegister::F12,
                FloatingPointRegister::F13,
                FloatingPointRegister::F14,
                FloatingPointRegister::F15,
                FloatingPointRegister::F16,
                FloatingPointRegister::F17,
                FloatingPointRegister::F18,
                FloatingPointRegister::F19,
                FloatingPointRegister::F20,
                FloatingPointRegister::F21,
                FloatingPointRegister::F22,
                FloatingPointRegister::F23,
                FloatingPointRegister::F24,
                FloatingPointRegister::F25,
                FloatingPointRegister::F26,
                FloatingPointRegister::F27,
                FloatingPointRegister::F28,
                FloatingPointRegister::F29,
                FloatingPointRegister::F30,
                FloatingPointRegister::F31,
            ] {
                if !s.trap_frame.floating_point_registers.contains(&fr) {
                    s.trap_frame.floating_point_registers.push(fr);
                }
            }

            if !s.trap_frame.csrs.contains(&Csr::Fcsr) {
                s.trap_frame.csrs.push(Csr::Fcsr);
            }
        }

        s
    }

    fn trap_frame_size(&self) -> isize {
        self.trap_frame.element_count() * self.xlen_bytes()
    }

    fn status_reg_offset(&self) -> isize {
        self.trap_frame.status_reg_idx() * self.xlen_bytes()
    }

    fn sp_reg_offset(&self) -> isize {
        self.trap_frame.sp_reg_idx() * self.xlen_bytes()
    }

    fn ra_reg_offset(&self) -> isize {
        self.trap_frame.ra_reg_idx() * self.xlen_bytes()
    }

    fn tp_reg_offset(&self) -> isize {
        self.trap_frame.tp_reg_idx() * self.xlen_bytes()
    }

    fn interrupted_frame_addr_offset(&self) -> isize {
        self.trap_frame.interrupted_frame_idx() * self.xlen_bytes()
    }

    fn rt_state_addr_offset(&self) -> isize {
        self.trap_frame.rt_flags_idx() * self.xlen_bytes()
    }

    pub fn max_hart_count(&self) -> usize {
        self.target_config.max_hart_count()
    }

    pub fn hart_stack_size(&self) -> usize {
        self.target_config.per_hart_stack_size()
    }

    fn boot_hart_rust_entrypoint(&self) -> &str {
        self.entrypoints.get(&EntrypointType::BootHart).unwrap()
    }

    fn nonboot_hart_rust_entrypoint(&self) -> &str {
        self.entrypoints.get(&EntrypointType::NonBootHart).unwrap()
    }

    fn trap_rust_entrypoint(&self) -> &str {
        self.entrypoints.get(&EntrypointType::Trap).unwrap()
    }

    fn custom_reset_entrypoint(&self) -> &str {
        self.entrypoints.get(&EntrypointType::CustomReset).unwrap()
    }

    fn stack_overflow_handle_entrypoint(&self) -> &str {
        self.entrypoints
            .get(&EntrypointType::StackOverflow)
            .unwrap()
    }

    fn csr_address_or_name(&self, csr: Csr) -> String {
        match csr {
            Csr::Other(addr, _name) => format!("0x{addr:x}"),
            _ => {
                if csr.is_mode_dependent() {
                    format!("{:#}{:#}", self.rv_mode(), csr)
                } else {
                    format!("{csr:#}")
                }
            }
        }
    }

    fn csr(&self, csr: Csr) -> String {
        if csr.is_mode_dependent() {
            format!("{:#}{:#}", self.rv_mode(), csr)
        } else {
            format!("{csr:#}")
        }
    }

    fn xlen_bytes(&self) -> isize {
        self.target_config.xlen_bytes()
    }

    fn word_prefix(&self) -> &str {
        self.target_config.xlen_word_prefix()
    }

    fn multihart_reset_handling_required(&self) -> bool {
        self.target_config.multihart_reset_handling_required()
    }

    fn current_mode_stack_offset(&self) -> isize {
        self.tp_block.current_mode_stack_idx() * self.xlen_bytes()
    }

    fn priv_ctx_offset(&self) -> isize {
        self.thread_ctx.priv_ctx_idx() * self.xlen_bytes()
    }

    fn return_addr_offset(&self) -> isize {
        self.tp_block.return_addr_idx() * self.xlen_bytes()
    }

    fn interrupted_mode_stack_offset(&self) -> isize {
        self.tp_block.interrupted_mode_stack_idx() * self.xlen_bytes()
    }

    fn interrupted_mode_tp_offset(&self) -> isize {
        self.tp_block.interrupted_mode_tp_idx() * self.xlen_bytes()
    }

    fn rust_entrypoint_offset(&self) -> isize {
        self.tp_block.rust_entrypoint_idx() * self.xlen_bytes()
    }

    fn boot_id_offset(&self) -> isize {
        self.tp_block.boot_id_idx() * self.xlen_bytes()
    }

    fn hart_id_offset(&self) -> isize {
        self.tp_block.hart_id_idx() * self.xlen_bytes()
    }

    fn context_addr_offset(&self) -> isize {
        self.tp_block.context_idx() * self.xlen_bytes()
    }

    fn tp_block_rt_flags_offset(&self) -> isize {
        self.tp_block.rt_flags_idx() * self.xlen_bytes()
    }

    fn tp_block_size(&self) -> isize {
        self.tp_block.reg_count() * self.xlen_bytes()
    }

    fn tp_block_trap_frame_offset(&self) -> isize {
        self.tp_block.trap_ctx_frame_idx() * self.xlen_bytes()
    }

    fn trap_frame_rust_struct_name(&self) -> String {
        self.trap_frame.rust_struct_name()
    }

    fn trap_frame_members(&self) -> Vec<String> {
        let mut members = Vec::new();
        for gr in &self.trap_frame.general_regs {
            members.push(gr.to_string());
        }
        for fr in &self.trap_frame.floating_point_registers {
            members.push(fr.to_string());
        }
        for csr in &self.trap_frame.csrs {
            members.push(self.csr(*csr));
        }
        for sv in &self.trap_frame.rt_state_values {
            members.push(sv.to_string());
        }
        members
    }

    fn is_multi_hart(&self) -> bool {
        self.target_config.is_multi_hart()
    }

    fn rv_mode(&self) -> RvMode {
        self.target_config.rv_mode()
    }

    fn rv_xlen(&self) -> RvXlen {
        self.target_config.rv_xlen()
    }

    fn is_skip_bss_clearing(&self) -> bool {
        self.skip_bss_clearing
    }

    fn needs_stack_overflow_detection(&self) -> bool {
        self.stack_overflow_detection
    }

    fn supports_atomic_extension(&self) -> bool {
        self.supports_atomic_extension
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum TpBlockMember {
    CurrentModeStack,
    InterruptedModeStack,
    InterruptedModeTp,
    RustEntrypoint,
    BootId,
    HartId,
    CurrContext,
    ReturnAddr,
    RtFlags,
    TrapCtx,
}

impl std::fmt::Display for TpBlockMember {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::CurrentModeStack => "current_mode_sp",
            Self::InterruptedModeStack => "interrupted_mode_sp",
            Self::InterruptedModeTp => "interrupted_mode_tp",
            Self::RustEntrypoint => "rust_entrypoint",
            Self::BootId => "boot_id",
            Self::HartId => "hart_id",
            Self::CurrContext => "curr_context",
            Self::ReturnAddr => "return_addr",
            Self::RtFlags => "rt_flags",
            Self::TrapCtx => "trap_ctx_frame",
        };
        write!(f, "{print_str}")
    }
}

#[derive(Debug)]
pub struct TpBlock {
    members: Vec<TpBlockMember>,
}

impl TpBlock {
    pub fn get_default() -> Self {
        Self {
            members: vec![
                TpBlockMember::CurrentModeStack,
                TpBlockMember::InterruptedModeStack,
                TpBlockMember::InterruptedModeTp,
                TpBlockMember::RustEntrypoint,
                TpBlockMember::BootId,
                TpBlockMember::HartId,
                TpBlockMember::CurrContext,
                TpBlockMember::ReturnAddr,
                TpBlockMember::RtFlags,
                TpBlockMember::TrapCtx,
            ],
        }
    }

    fn member_idx(&self, ty: TpBlockMember) -> isize {
        for (idx, member) in self.members.iter().enumerate() {
            if *member == ty {
                return idx as isize;
            }
        }
        unreachable!()
    }

    fn current_mode_stack_idx(&self) -> isize {
        self.member_idx(TpBlockMember::CurrentModeStack)
    }

    fn return_addr_idx(&self) -> isize {
        self.member_idx(TpBlockMember::ReturnAddr)
    }

    fn interrupted_mode_stack_idx(&self) -> isize {
        self.member_idx(TpBlockMember::InterruptedModeStack)
    }

    fn interrupted_mode_tp_idx(&self) -> isize {
        self.member_idx(TpBlockMember::InterruptedModeTp)
    }

    fn rust_entrypoint_idx(&self) -> isize {
        self.member_idx(TpBlockMember::RustEntrypoint)
    }

    fn boot_id_idx(&self) -> isize {
        self.member_idx(TpBlockMember::BootId)
    }

    fn hart_id_idx(&self) -> isize {
        self.member_idx(TpBlockMember::HartId)
    }

    fn context_idx(&self) -> isize {
        self.member_idx(TpBlockMember::CurrContext)
    }

    fn rt_flags_idx(&self) -> isize {
        self.member_idx(TpBlockMember::RtFlags)
    }

    fn trap_ctx_frame_idx(&self) -> isize {
        self.member_idx(TpBlockMember::TrapCtx)
    }

    fn reg_count(&self) -> isize {
        self.members.len() as isize
    }

    fn rust_struct_name(&self) -> String {
        "TpBlock".to_string()
    }

    fn members(&self) -> Vec<String> {
        let mut members = Vec::new();

        for member in &self.members {
            members.push(member.to_string());
        }

        members
    }
}

// Make ThreadContext independent of XLEN
#[derive(Debug, Eq, PartialEq)]
pub enum ThreadContextMember {
    PrivCtx,
}

impl std::fmt::Display for ThreadContextMember {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::PrivCtx => "priv_ctx",
        };
        write!(f, "{print_str}")
    }
}

#[derive(Debug)]
pub struct ThreadContext {
    members: Vec<ThreadContextMember>,
}
impl ThreadContext {
    pub fn get_default() -> Self {
        Self {
            members: vec![ThreadContextMember::PrivCtx],
        }
    }

    fn member_idx(&self, ty: ThreadContextMember) -> isize {
        for (idx, member) in self.members.iter().enumerate() {
            if *member == ty {
                return idx as isize;
            }
        }
        unreachable!()
    }

    fn priv_ctx_idx(&self) -> isize {
        self.member_idx(ThreadContextMember::PrivCtx)
    }
}

#[derive(Debug)]
pub struct TrapFrame {
    pub general_regs: Vec<GeneralRegister>,
    pub floating_point_registers: Vec<FloatingPointRegister>,
    pub csrs: Vec<Csr>,
    pub rt_state_values: Vec<RtStateValue>,
}

impl TrapFrame {
    fn element_count(&self) -> isize {
        (self.general_regs.len()
            + self.floating_point_registers.len()
            + self.csrs.len()
            + self.rt_state_values.len()) as isize
    }

    fn gr_start_idx(&self) -> isize {
        // General registers are stashed at the beginning of trap frame
        0
    }

    fn fr_start_idx(&self) -> isize {
        // Floating point registers are stashed after the general purpose registers
        self.general_regs.len() as isize
    }

    fn csr_start_idx(&self) -> isize {
        // CSRs are placed after general regs and floating point regs in trap frame
        (self.general_regs.len() + self.floating_point_registers.len()) as isize
    }

    fn rt_state_start_idx(&self) -> isize {
        // runtime-state data is placed after csr regs in trap frame
        (self.general_regs.len() + self.floating_point_registers.len() + self.csrs.len()) as isize
    }

    fn gr_idx(&self, reg: GeneralRegister) -> isize {
        for (idx, gr) in self.general_regs.iter().enumerate() {
            if *gr == reg {
                return idx as isize + self.gr_start_idx();
            }
        }
        unreachable!()
    }

    fn csr_idx(&self, reg: Csr) -> isize {
        for (idx, csr) in self.csrs.iter().enumerate() {
            if *csr == reg {
                return idx as isize + self.csr_start_idx();
            }
        }

        unreachable!();
    }

    fn rt_state_idx(&self, val: RtStateValue) -> isize {
        for (idx, sv) in self.rt_state_values.iter().enumerate() {
            if *sv == val {
                return idx as isize + self.rt_state_start_idx();
            }
        }
        unreachable!()
    }

    fn status_reg_idx(&self) -> isize {
        self.csr_idx(Csr::Status)
    }

    fn interrupted_frame_idx(&self) -> isize {
        self.rt_state_idx(RtStateValue::InterruptedTrapFrameAddr)
    }

    fn rt_flags_idx(&self) -> isize {
        self.rt_state_idx(RtStateValue::RtFlags)
    }

    fn sp_reg_idx(&self) -> isize {
        self.gr_idx(GeneralRegister::Sp)
    }

    fn ra_reg_idx(&self) -> isize {
        self.gr_idx(GeneralRegister::Ra)
    }

    fn tp_reg_idx(&self) -> isize {
        self.gr_idx(GeneralRegister::Tp)
    }

    pub fn get_default() -> Self {
        Self {
            general_regs: vec![
                GeneralRegister::Ra,
                GeneralRegister::Sp,
                GeneralRegister::Gp,
                GeneralRegister::Tp,
                GeneralRegister::T0,
                GeneralRegister::T1,
                GeneralRegister::T2,
                GeneralRegister::S0,
                GeneralRegister::S1,
                GeneralRegister::A0,
                GeneralRegister::A1,
                GeneralRegister::A2,
                GeneralRegister::A3,
                GeneralRegister::A4,
                GeneralRegister::A5,
                GeneralRegister::A6,
                GeneralRegister::A7,
                GeneralRegister::S2,
                GeneralRegister::S3,
                GeneralRegister::S4,
                GeneralRegister::S5,
                GeneralRegister::S6,
                GeneralRegister::S7,
                GeneralRegister::S8,
                GeneralRegister::S9,
                GeneralRegister::S10,
                GeneralRegister::S11,
                GeneralRegister::T3,
                GeneralRegister::T4,
                GeneralRegister::T5,
                GeneralRegister::T6,
            ],
            floating_point_registers: vec![],
            csrs: vec![Csr::Status, Csr::Epc, Csr::Tval, Csr::Cause],
            rt_state_values: vec![
                RtStateValue::RtFlags,
                RtStateValue::InterruptedTrapFrameAddr,
            ],
        }
    }

    fn rust_struct_name(&self) -> String {
        "TrapFrame".to_string()
    }
}

#[derive(Debug, PartialEq)]
pub enum RtStateValue {
    RtFlags,
    InterruptedTrapFrameAddr,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Csr {
    Ie,
    Mcounteren,
    Menvcfg,
    Mideleg,
    Medeleg,
    Mhartid,
    Status,
    Epc,
    Scratch,
    Tval,
    Cause,
    Tvec,
    Satp,
    Fcsr,
    // The address and name of the CSR
    Other(usize, &'static str),
}

impl Csr {
    fn is_mode_dependent(&self) -> bool {
        match self {
            Self::Mhartid
            | Self::Other(_, _)
            | Self::Mideleg
            | Self::Medeleg
            | Self::Satp
            | Self::Menvcfg
            | Self::Mcounteren
            | Self::Fcsr => false,
            Self::Ie
            | Self::Status
            | Self::Epc
            | Self::Scratch
            | Self::Tval
            | Self::Cause
            | Self::Tvec => true,
        }
    }

    fn restore_from_trap_frame(&self) -> bool {
        // matches! macro returns whether the given expression matches any of
        // the given patterns. In our case, Xcause and Xtval don't need to be
        // restored from trap frame because they are set on every entry into
        // that mode, restoring those CSRs isn't required when returning back
        // from the trap handler
        !matches!(self, Self::Cause | Self::Tval)
    }
}

impl std::fmt::Display for Csr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::Ie => "ie",
            Self::Mcounteren => "mcounteren",
            Self::Menvcfg => "menvcfg",
            Self::Mideleg => "mideleg",
            Self::Medeleg => "medeleg",
            Self::Mhartid => "mhartid",
            Self::Satp => "satp",
            Self::Status => "status",
            Self::Epc => "epc",
            Self::Scratch => "scratch",
            Self::Tval => "tval",
            Self::Cause => "cause",
            Self::Tvec => "tvec",
            Self::Fcsr => "fcsr",
            Self::Other(_addr, name) => name,
        };
        write!(f, "{print_str}")
    }
}

impl std::fmt::Display for RtStateValue {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::InterruptedTrapFrameAddr => "int_frame",
            Self::RtFlags => "rt_flags",
        };
        write!(f, "{print_str}")
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum GeneralRegister {
    Zero,
    Ra,
    Sp,
    Gp,
    Tp,
    T0,
    T1,
    T2,
    S0,
    S1,
    A0,
    A1,
    A2,
    A3,
    A4,
    A5,
    A6,
    A7,
    S2,
    S3,
    S4,
    S5,
    S6,
    S7,
    S8,
    S9,
    S10,
    S11,
    T3,
    T4,
    T5,
    T6,
}

impl std::fmt::Display for GeneralRegister {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::Zero => "zero",
            Self::Ra => "ra",
            Self::Sp => "sp",
            Self::Gp => "gp",
            Self::Tp => "tp",
            Self::T0 => "t0",
            Self::T1 => "t1",
            Self::T2 => "t2",
            Self::S0 => "s0",
            Self::S1 => "s1",
            Self::A0 => "a0",
            Self::A1 => "a1",
            Self::A2 => "a2",
            Self::A3 => "a3",
            Self::A4 => "a4",
            Self::A5 => "a5",
            Self::A6 => "a6",
            Self::A7 => "a7",
            Self::S2 => "s2",
            Self::S3 => "s3",
            Self::S4 => "s4",
            Self::S5 => "s5",
            Self::S6 => "s6",
            Self::S7 => "s7",
            Self::S8 => "s8",
            Self::S9 => "s9",
            Self::S10 => "s10",
            Self::S11 => "s11",
            Self::T3 => "t3",
            Self::T4 => "t4",
            Self::T5 => "t5",
            Self::T6 => "t6",
        };
        write!(f, "{print_str}")
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FloatingPointRegister {
    F0,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
    F25,
    F26,
    F27,
    F28,
    F29,
    F30,
    F31,
}

impl std::fmt::Display for FloatingPointRegister {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::F0 => "f0",
            Self::F1 => "f1",
            Self::F2 => "f2",
            Self::F3 => "f3",
            Self::F4 => "f4",
            Self::F5 => "f5",
            Self::F6 => "f6",
            Self::F7 => "f7",
            Self::F8 => "f8",
            Self::F9 => "f9",
            Self::F10 => "f10",
            Self::F11 => "f11",
            Self::F12 => "f12",
            Self::F13 => "f13",
            Self::F14 => "f14",
            Self::F15 => "f15",
            Self::F16 => "f16",
            Self::F17 => "f17",
            Self::F18 => "f18",
            Self::F19 => "f19",
            Self::F20 => "f20",
            Self::F21 => "f21",
            Self::F22 => "f22",
            Self::F23 => "f23",
            Self::F24 => "f24",
            Self::F25 => "f25",
            Self::F26 => "f26",
            Self::F27 => "f27",
            Self::F28 => "f28",
            Self::F29 => "f29",
            Self::F30 => "f30",
            Self::F31 => "f31",
        };
        write!(f, "{print_str}")
    }
}

#[derive(Debug)]
pub enum LinkerOption {
    Push,
    Pop,
    NoRelax,
}

impl std::fmt::Display for LinkerOption {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Self::Push => "push",
            Self::Pop => "pop",
            Self::NoRelax => "norelax",
        };
        write!(f, "{print_str}")
    }
}

#[derive(Debug)]
enum AsmSentence {
    Section(String, Option<String>),              // (section name, flags)
    GlobalEntrypoint(String),                     // (entrypoint name)
    Csrw(Csr, GeneralRegister),                   // (csr, rs)
    Csrr(GeneralRegister, Csr),                   // (rd, csr)
    Csrrw(GeneralRegister, Csr, GeneralRegister), // (rd, csr, rs)
    Csrc(Csr, GeneralRegister),                   // (csr, rs)
    Csrs(Csr, GeneralRegister),                   // (csr, rs)
    LinkerOption(LinkerOption),                   // (option)
    La(GeneralRegister, String),                  // (rd, symbol)
    Li(GeneralRegister, usize),                   // (rd, imm)
    Bgeu(GeneralRegister, GeneralRegister, String), //  (rs1, rs2, label)
    Bltu(GeneralRegister, GeneralRegister, String), // (rs1, rs2, label)
    Beq(GeneralRegister, GeneralRegister, String), // (rs1, rs2, label)
    Bne(GeneralRegister, GeneralRegister, String), // (rs1, rs2, label)
    Beqz(GeneralRegister, String),                // (rs, label)
    Bnez(GeneralRegister, String),                // (rs, label)
    Label(String),                                // (label)
    Sfence(GeneralRegister, GeneralRegister),     // (rs1, rs2)
    Store(GeneralRegister, GeneralRegister, isize), // (rs2, rs1, offset)
    Load(GeneralRegister, GeneralRegister, isize), // (rd, rs, offset)
    Addi(GeneralRegister, GeneralRegister, isize), // (rd, rs, imm)
    Xori(GeneralRegister, GeneralRegister, isize), // (rd, rs, imm)
    Or(GeneralRegister, GeneralRegister, GeneralRegister),
    FloatStore(FloatingPointRegister, GeneralRegister, isize), // (rs2, rs1, offset)
    FloatLoad(FloatingPointRegister, GeneralRegister, isize),  // (rd, rs, offset)
    MoveToFloat(FloatingPointRegister, GeneralRegister),       // (fd, rs)
    Wfi,
    J(String),                                              // (label)
    Jal(String),                                            // (label)
    Jr(GeneralRegister),                                    // (rs)
    Jalr(GeneralRegister, GeneralRegister, isize),          // (rd, rs1, offset)
    Comment(String),                                        // (comment)
    Add(GeneralRegister, GeneralRegister, GeneralRegister), // (rd, rs1, rs2)
    Sub(GeneralRegister, GeneralRegister, GeneralRegister), // (rd, rs1, rs2)
    Mul(GeneralRegister, GeneralRegister, GeneralRegister), // (rd, rs1, rs2)
    Dword(u64),                                             // (val)
    Word(u32),                                              // (val)
    EndSection,
    Amoadd(GeneralRegister, GeneralRegister, GeneralRegister), // (rd, rs1, rs2)
    Ret,
    Moderet,
    Rept(usize), // (count)
    EndRept,
    And(GeneralRegister, GeneralRegister, GeneralRegister), // (rd, rs1, rs2)
    Andi(GeneralRegister, GeneralRegister, isize),          // (rd, rs1, imm)
    Align(usize),                                           // (alignment in bytes)
    Attribute(String, String),                              // (name, value)
    Sc(GeneralRegister, GeneralRegister, GeneralRegister),  // (rd, rs2, rs1)
}

impl AsmSentence {
    fn generate(&self, fw: &FileWriter, rt_config: &RtConfig) {
        match self {
            Self::Section(section_name, flags) => {
                if let Some(flags) = flags {
                    fw.add_line(&format!(".section {section_name:#}, {flags:?}"));
                } else {
                    fw.add_line(&format!(".section {section_name:#}"));
                }
            }
            Self::EndSection => fw.end_block(),
            Self::GlobalEntrypoint(entrypoint_name) => {
                fw.add_line(&format!(".global {entrypoint_name:#}"));
                fw.label(entrypoint_name);
            }
            Self::Csrw(csr, rs) => fw.add_line(&format!(
                "csrw {:#}, {:#}",
                rt_config.csr_address_or_name(*csr),
                rs
            )),
            Self::Csrs(csr, rs) => fw.add_line(&format!(
                "csrs {:#}, {:#}",
                rt_config.csr_address_or_name(*csr),
                rs
            )),
            Self::Csrc(csr, rs) => fw.add_line(&format!(
                "csrc {:#}, {:#}",
                rt_config.csr_address_or_name(*csr),
                rs
            )),
            Self::Csrr(rd, csr) => fw.add_line(&format!(
                "csrr {:#}, {:#}",
                rd,
                rt_config.csr_address_or_name(*csr)
            )),
            Self::Csrrw(rd, csr, rs) => fw.add_line(&format!(
                "csrrw {:#}, {:#}, {:#}",
                rd,
                rt_config.csr(*csr),
                rs
            )),
            Self::LinkerOption(option) => fw.add_line(&format!(".option {option:#}")),
            Self::La(rd, symbol) => fw.add_line(&format!("la {rd:#}, {symbol:#}")),
            Self::Li(rd, imm) => fw.add_line(&format!("li {rd:#}, {imm:#}")),
            Self::Bgeu(rs1, rs2, label) => {
                fw.add_line(&format!("bgeu {rs1:#}, {rs2:#}, {label:#}"))
            }
            Self::Bltu(rs1, rs2, label) => {
                fw.add_line(&format!("bltu {rs1:#}, {rs2:#}, {label:#}"))
            }
            Self::Beq(rs1, rs2, label) => fw.add_line(&format!("beq {rs1:#}, {rs2:#}, {label:#}")),
            Self::Bne(rs1, rs2, label) => fw.add_line(&format!("bne {rs1:#}, {rs2:#}, {label:#}")),
            Self::Beqz(rs, label) => fw.add_line(&format!("beqz {rs:#}, {label:#}")),
            Self::Bnez(rs, label) => fw.add_line(&format!("bnez {rs:#}, {label:#}")),
            Self::Label(label) => fw.label(&format!("{label:#}")),
            Self::Sfence(rs1, rs2) => fw.add_line(&format!("sfence.vma {rs1:#}, {rs2:#}")),
            Self::Store(rs2, rs1, offset) => {
                if *offset == 0 {
                    fw.add_line(&format!(
                        "s{:#} {:#}, ({:#})",
                        rt_config.word_prefix(),
                        rs2,
                        rs1
                    ));
                } else {
                    fw.add_line(&format!(
                        "s{:#} {:#}, {:#}({:#})",
                        rt_config.word_prefix(),
                        rs2,
                        offset,
                        rs1
                    ));
                }
            }
            Self::Load(rd, rs, offset) => {
                if *offset == 0 {
                    fw.add_line(&format!(
                        "l{:#} {:#}, ({:#})",
                        rt_config.word_prefix(),
                        rd,
                        rs
                    ));
                } else {
                    fw.add_line(&format!(
                        "l{:#} {:#}, {:#}({:#})",
                        rt_config.word_prefix(),
                        rd,
                        offset,
                        rs
                    ));
                }
            }
            Self::Addi(rd, rs, imm) => fw.add_line(&format!("addi {rd:#}, {rs:#}, {imm:#}")),
            Self::Xori(rd, rs, imm) => fw.add_line(&format!("xori {rd:#}, {rs:#}, {imm:#}")),
            Self::Or(rd, rs1, rs2) => fw.add_line(&format!("or {rd:#}, {rs1:#}, {rs2:#}")),
            Self::FloatStore(rs2, rs1, offset) => {
                if *offset == 0 {
                    fw.add_line(&format!(
                        "fs{:#} {:#}, ({:#})",
                        rt_config.word_prefix(),
                        rs2,
                        rs1
                    ));
                } else {
                    fw.add_line(&format!(
                        "fs{:#} {:#}, {:#}({:#})",
                        rt_config.word_prefix(),
                        rs2,
                        offset,
                        rs1
                    ));
                }
            }
            Self::FloatLoad(rd, rs, offset) => {
                if *offset == 0 {
                    fw.add_line(&format!(
                        "fl{:#} {:#}, ({:#})",
                        rt_config.word_prefix(),
                        rd,
                        rs
                    ));
                } else {
                    fw.add_line(&format!(
                        "fl{:#} {:#}, {:#}({:#})",
                        rt_config.word_prefix(),
                        rd,
                        offset,
                        rs
                    ));
                }
            }
            Self::MoveToFloat(fd, rs) => fw.add_line(&format!("fmv.d.x {fd:#}, {rs:#}")),
            Self::Wfi => fw.add_line("wfi"),
            Self::J(label) => fw.add_line(&format!("j {label:#}")),
            Self::Jal(label) => fw.add_line(&format!("jal {label:#}")),
            Self::Jr(rs) => fw.add_line(&format!("jr {rs:#}")),
            Self::Jalr(rd, rs1, offset) => {
                fw.add_line(&format!("jalr {rd:#}, {rs1:#}, {offset:#}"))
            }
            Self::Comment(comment) => fw.add_line(&format!("// {comment:#}")),
            Self::Add(rd, rs1, rs2) => fw.add_line(&format!("add {rd:#}, {rs1:#}, {rs2:#}")),
            Self::Sub(rd, rs1, rs2) => fw.add_line(&format!("sub {rd:#}, {rs1:#}, {rs2:#}")),
            Self::Mul(rd, rs1, rs2) => fw.add_line(&format!("mul {rd:#}, {rs1:#}, {rs2:#}")),
            Self::Dword(val) => fw.add_line(&format!(".dword {val:#}")),
            Self::Word(val) => fw.add_line(&format!(".word {val:#}")),
            Self::Amoadd(rd, rs1, rs2) => fw.add_line(&format!(
                "amoadd.{:#} {:#}, {:#}, ({:#})",
                rt_config.word_prefix(),
                rd,
                rs2,
                rs1
            )),
            Self::Ret => fw.add_line("ret"),
            Self::Moderet => fw.add_line(&format!("{:#}ret", rt_config.rv_mode())),
            Self::Rept(count) => fw.add_line(&format!(".rept {count:#}")),
            Self::EndRept => fw.add_line(".endr"),
            Self::And(rd, rs1, rs2) => fw.add_line(&format!("and {rd:#}, {rs1:#}, {rs2:#}")),
            Self::Andi(rd, rs, imm) => {
                fw.add_line(&format!("andi {rd:#}, {rs:#}, {imm:#}"));
            }
            Self::Align(alignment) => {
                fw.goto_next_line();
                fw.add_line(&format!(".align {alignment:#}"));
            }
            Self::Attribute(name, value) => {
                fw.add_line(&format!(".attribute {name:#}, {value:?}"));
            }
            Self::Sc(rd, rs2, rs1) => {
                fw.add_line(&format!(
                    "sc.{:#} {:#}, {:#}, ({:#})",
                    rt_config.word_prefix(),
                    rd,
                    rs2,
                    rs1
                ));
            }
        }
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Copy, Clone)]
pub enum LabelType {
    ParkHart,
    SecondaryStart,
    BootIdxVariable,
    ResetStart,
    RestoreTrapFrame,
    CreateTrapFrame,
    HandleTrap,
    ThreadPointerBlock,
    JumpToRustEntrypoint,
    BssInitDone,
    CustomResetEntryPoint,
    ProtectStack,
    GetTrapAddr,
}

#[derive(Debug, Hash, Eq, PartialEq)]
pub enum NamedReg {
    BootId,
    HartId,
}

#[derive(Debug)]
struct AsmBuilder<'a> {
    rt_config: &'a RtConfig,
    next_label: RefCell<usize>,
    sentences: RefCell<Vec<AsmSentence>>,
    free_general_regs: RefCell<Vec<GeneralRegister>>,
    label_map: RefCell<HashMap<LabelType, String>>,
    named_regs: RefCell<HashMap<NamedReg, GeneralRegister>>,
}

impl<'a> AsmBuilder<'a> {
    fn new(rt_config: &'a RtConfig) -> Self {
        let ab = Self {
            rt_config,
            next_label: RefCell::new(1),
            sentences: RefCell::new(Vec::new()),
            free_general_regs: RefCell::new(Vec::new()),
            label_map: RefCell::new(HashMap::new()),
            named_regs: RefCell::new(HashMap::new()),
        };
        ab.comment(&auto_generate_banner());
        ab
    }

    fn assign_free_reg_pool(&self, regs: &[GeneralRegister]) {
        self.free_general_regs.borrow_mut().extend_from_slice(regs);
    }

    fn drain_free_reg_pool(&self) {
        self.free_general_regs.borrow_mut().truncate(0);
    }

    fn init_default_free_reg_pool(&self) {
        self.drain_free_reg_pool();
        self.assign_free_reg_pool(&[
            GeneralRegister::T0,
            GeneralRegister::T1,
            GeneralRegister::T2,
            GeneralRegister::T3,
            GeneralRegister::T4,
            GeneralRegister::T5,
            GeneralRegister::T6,
        ]);
    }

    fn add_named_reg(&self, name: NamedReg, reg: GeneralRegister) {
        self.named_regs.borrow_mut().insert(name, reg);
    }

    fn remove_named_reg(&self, name: NamedReg) {
        self.named_regs.borrow_mut().remove(&name);
    }

    fn get_named_reg(&self, name: NamedReg) -> GeneralRegister {
        *self.named_regs.borrow().get(&name).unwrap()
    }

    fn release_id_regs(&self) {
        self.release_boot_id_reg();
        self.release_hart_id_reg();
    }

    fn allocate_id_regs(&self) {
        self.allocate_reg_for_boot_id();
        self.allocate_reg_for_hart_id();
    }

    fn allocate_reg_for_boot_id(&self) {
        let reg = self.get_free_reg();
        self.add_named_reg(NamedReg::BootId, reg);
    }

    fn release_boot_id_reg(&self) {
        let reg = self.get_boot_id_reg();
        self.release_reg(reg);
        self.remove_named_reg(NamedReg::BootId);
    }

    fn get_boot_id_reg(&self) -> GeneralRegister {
        self.get_named_reg(NamedReg::BootId)
    }

    fn allocate_reg_for_hart_id(&self) {
        let reg = self.get_free_reg();
        self.add_named_reg(NamedReg::HartId, reg);
    }

    fn release_hart_id_reg(&self) {
        let reg = self.get_hart_id_reg();
        self.release_reg(reg);
        self.remove_named_reg(NamedReg::HartId);
    }

    fn get_hart_id_reg(&self) -> GeneralRegister {
        self.get_named_reg(NamedReg::HartId)
    }

    fn add_label_to_map(&self, ty: LabelType, label: &str) {
        self.label_map.borrow_mut().insert(ty, label.to_string());
    }

    fn add_labels(&self, labels: &[(LabelType, &str)]) {
        labels
            .iter()
            .for_each(|(label_ty, label_name)| self.add_label_to_map(*label_ty, label_name));
    }

    fn get_label_from_map(&self, ty: LabelType) -> String {
        self.label_map.borrow().get(&ty).unwrap().to_string()
    }

    fn get_free_reg(&self) -> GeneralRegister {
        if self.free_general_regs.borrow().is_empty() {
            panic!("out of free general registers!");
        }

        self.free_general_regs.borrow_mut().pop().unwrap()
    }

    fn release_reg(&self, reg: GeneralRegister) {
        self.free_general_regs.borrow_mut().push(reg);
    }

    fn generate(&self, fw: &FileWriter) {
        for sentence in self.sentences.borrow().iter() {
            sentence.generate(fw, self.rt_config);
        }
    }

    fn next_label(&self) -> String {
        let mut label_ptr = self.next_label.borrow_mut();
        let label = *label_ptr;
        *label_ptr += 1;

        format!("{label:#}")
    }

    fn add_sentence(&self, sentence: AsmSentence) {
        self.sentences.borrow_mut().push(sentence);
    }

    fn text_section_flags(&self) -> String {
        "ax".to_string()
    }

    fn global_entrypoint(&self, section: &str) {
        self.section(section, Some(self.text_section_flags()));
        self.add_sentence(AsmSentence::GlobalEntrypoint(
            self.get_label_from_map(LabelType::ResetStart),
        ));
    }

    fn global_function(&self, fn_name: &str) {
        self.section(&text_default_section(), Some(self.text_section_flags()));
        self.add_sentence(AsmSentence::GlobalEntrypoint(fn_name.to_string()));
    }

    fn section(&self, section: &str, flags: Option<String>) {
        self.add_sentence(AsmSentence::Section(section.to_string(), flags));
    }

    fn csrw(&self, csr: Csr, rs: GeneralRegister) {
        self.add_sentence(AsmSentence::Csrw(csr, rs));
    }

    fn csrs(&self, csr: Csr, rs: GeneralRegister) {
        self.add_sentence(AsmSentence::Csrs(csr, rs));
    }

    fn csrc(&self, csr: Csr, rs: GeneralRegister) {
        self.add_sentence(AsmSentence::Csrc(csr, rs));
    }

    fn csrw_zero(&self, csr: Csr) {
        self.add_sentence(AsmSentence::Csrw(csr, GeneralRegister::Zero));
    }

    fn csrr(&self, rd: GeneralRegister, csr: Csr) {
        self.add_sentence(AsmSentence::Csrr(rd, csr));
    }

    fn csrrw(&self, rd: GeneralRegister, csr: Csr, rs: GeneralRegister) {
        self.add_sentence(AsmSentence::Csrrw(rd, csr, rs));
    }

    fn option_push(&self) {
        self.add_sentence(AsmSentence::LinkerOption(LinkerOption::Push));
    }

    fn option_pop(&self) {
        self.add_sentence(AsmSentence::LinkerOption(LinkerOption::Pop));
    }

    fn option_norelax(&self) {
        self.add_sentence(AsmSentence::LinkerOption(LinkerOption::NoRelax));
    }

    fn la(&self, rd: GeneralRegister, symbol: &str) {
        self.add_sentence(AsmSentence::La(rd, symbol.to_string()));
    }

    fn li_unconstrained(&self, rd: GeneralRegister, imm: usize) {
        self.add_sentence(AsmSentence::Li(rd, imm));
    }

    fn li_constrained(&self, rd: GeneralRegister, imm: usize) {
        assert!(
            (-2048..=2047).contains(&(imm as isize)),
            "Immediate value out of range"
        );
        self.add_sentence(AsmSentence::Li(rd, imm));
    }

    fn bgeu(&self, rs1: GeneralRegister, rs2: GeneralRegister, label: &str) {
        self.add_sentence(AsmSentence::Bgeu(rs1, rs2, label.to_string()));
    }

    fn bltu(&self, rs1: GeneralRegister, rs2: GeneralRegister, label: &str) {
        self.add_sentence(AsmSentence::Bltu(rs1, rs2, label.to_string()));
    }

    fn beq(&self, rs1: GeneralRegister, rs2: GeneralRegister, label: &str) {
        self.add_sentence(AsmSentence::Beq(rs1, rs2, label.to_string()));
    }

    fn bne(&self, rs1: GeneralRegister, rs2: GeneralRegister, label: &str) {
        self.add_sentence(AsmSentence::Bne(rs1, rs2, label.to_string()));
    }

    fn beqz(&self, rs: GeneralRegister, label: &str) {
        self.add_sentence(AsmSentence::Beqz(rs, label.to_string()));
    }

    fn bnez(&self, rs: GeneralRegister, label: &str) {
        self.add_sentence(AsmSentence::Bnez(rs, label.to_string()));
    }

    fn label(
        &self,
        label: &str,
        alignment: Option<usize>,
        section: Option<&str>,
        section_flags: Option<String>,
    ) {
        if let Some(alignment) = alignment {
            self.align(alignment);
        }
        if let Some(section) = section {
            self.section(section, section_flags);
        }
        self.add_sentence(AsmSentence::Label(label.to_string()));
    }

    fn load(&self, rd: GeneralRegister, rs: GeneralRegister, offset: isize) {
        self.add_sentence(AsmSentence::Load(rd, rs, offset));
    }

    fn store(&self, rs2: GeneralRegister, rs1: GeneralRegister, offset: isize) {
        self.add_sentence(AsmSentence::Store(rs2, rs1, offset));
    }

    fn sfence(&self, rs1: GeneralRegister, rs2: GeneralRegister) {
        self.add_sentence(AsmSentence::Sfence(rs1, rs2));
    }

    fn fload(&self, rd: FloatingPointRegister, rs: GeneralRegister, offset: isize) {
        self.add_sentence(AsmSentence::FloatLoad(rd, rs, offset));
    }

    fn move_to_float(&self, fd: FloatingPointRegister, rs1: GeneralRegister) {
        self.add_sentence(AsmSentence::MoveToFloat(fd, rs1))
    }

    fn fstore(&self, rs2: FloatingPointRegister, rs1: GeneralRegister, offset: isize) {
        self.add_sentence(AsmSentence::FloatStore(rs2, rs1, offset));
    }

    fn store_zero(&self, rs1: GeneralRegister) {
        self.store(GeneralRegister::Zero, rs1, 0);
    }

    fn addi(&self, rd: GeneralRegister, rs: GeneralRegister, imm: isize) {
        assert!(
            (-2048..=2047).contains(&imm),
            "Immediate value out of range"
        );
        self.add_sentence(AsmSentence::Addi(rd, rs, imm));
    }

    fn xori(&self, rd: GeneralRegister, rs: GeneralRegister, imm: isize) {
        assert!(
            (-2048..=2047).contains(&imm),
            "Immediate value out of range"
        );
        self.add_sentence(AsmSentence::Xori(rd, rs, imm));
    }

    fn or(&self, rd: GeneralRegister, rs1: GeneralRegister, rs2: GeneralRegister) {
        self.add_sentence(AsmSentence::Or(rd, rs1, rs2))
    }

    fn wfi(&self) {
        self.add_sentence(AsmSentence::Wfi);
    }

    fn j(&self, label: &str) {
        self.add_sentence(AsmSentence::J(label.to_string()));
    }

    fn jal(&self, label: &str) {
        self.add_sentence(AsmSentence::Jal(label.to_string()));
    }

    fn jr(&self, rs: GeneralRegister) {
        self.add_sentence(AsmSentence::Jr(rs));
    }

    fn jalr(&self, rd: GeneralRegister, rs1: GeneralRegister, offset: isize) {
        self.add_sentence(AsmSentence::Jalr(rd, rs1, offset));
    }

    fn comment(&self, comment: &str) {
        self.add_sentence(AsmSentence::Comment(comment.to_string()));
    }

    fn add(&self, rd: GeneralRegister, rs1: GeneralRegister, rs2: GeneralRegister) {
        self.add_sentence(AsmSentence::Add(rd, rs1, rs2));
    }

    fn sub(&self, rd: GeneralRegister, rs1: GeneralRegister, rs2: GeneralRegister) {
        self.add_sentence(AsmSentence::Sub(rd, rs1, rs2));
    }

    fn mov(&self, rd: GeneralRegister, rs: GeneralRegister) {
        self.add_sentence(AsmSentence::Add(rd, rs, GeneralRegister::Zero));
    }

    fn mul(&self, rd: GeneralRegister, rs1: GeneralRegister, rs2: GeneralRegister) {
        self.add_sentence(AsmSentence::Mul(rd, rs1, rs2));
    }

    fn dword(&self, val: u64) {
        self.add_sentence(AsmSentence::Dword(val));
    }

    fn word(&self, val: u32) {
        self.add_sentence(AsmSentence::Word(val));
    }

    fn xword(&self, val: usize) {
        if self.rt_config.xlen_bytes() == 8 {
            self.dword(val as u64);
        } else {
            self.word(val as u32);
        }
    }

    fn end_section(&self) {
        self.add_sentence(AsmSentence::EndSection);
    }

    fn amoadd(&self, rd: GeneralRegister, rs1: GeneralRegister, rs2: GeneralRegister) {
        self.add_sentence(AsmSentence::Amoadd(rd, rs1, rs2));
    }

    fn ret(&self) {
        self.add_sentence(AsmSentence::Ret);
    }

    fn mode_ret(&self) {
        self.add_sentence(AsmSentence::Moderet);
    }

    fn sc(&self, rd: GeneralRegister, rs2: GeneralRegister, rs1: GeneralRegister) {
        self.add_sentence(AsmSentence::Sc(rd, rs2, rs1));
    }

    fn rept(&self, count: usize, val: usize) {
        self.add_sentence(AsmSentence::Rept(
            count / self.rt_config.xlen_bytes() as usize,
        ));
        if self.rt_config.xlen_bytes() == 8 {
            self.dword(val as u64);
        } else {
            self.word(val as u32);
        }
        self.add_sentence(AsmSentence::EndRept);
    }

    fn and(&self, rd: GeneralRegister, rs1: GeneralRegister, rs2: GeneralRegister) {
        self.add_sentence(AsmSentence::And(rd, rs1, rs2));
    }

    fn andi(&self, rd: GeneralRegister, rs: GeneralRegister, imm: isize) {
        assert!(
            (-2048..=2047).contains(&imm),
            "Immediate value out of range"
        );
        self.add_sentence(AsmSentence::Andi(rd, rs, imm));
    }

    fn align(&self, alignment_bytes: usize) {
        self.add_sentence(AsmSentence::Align(alignment_bytes));
    }

    fn preamble(&self) {
        if self.rt_config.rv_xlen() == RvXlen::Rv64 {
            // Workaround required to silence the compiler warnings for the generated code.
            // Since we are using AMO instructions, the compiler is incorrectly printing out non-fatal errors.
            // See https://github.com/rust-lang/rust/issues/80608. Defaulting to rv64gc on rv64 platforms
            // seems to silence these prints. Adding this workaround here until the compiler bug gets fixed.
            self.add_sentence(AsmSentence::Attribute(
                "arch".to_string(),
                "rv64gc".to_string(),
            ));
        }
    }

    // Set a bit (corresponding to passed flag) in given register `reg`.
    // All other rt_flag bits set in `reg` are cleared.
    fn set_rt_flag_bit(&self, reg: GeneralRegister, flag: RtFlagBit) {
        self.addi(reg, GeneralRegister::Zero, flag.as_mask());
    }

    // Write value in given register `reg` to rt_flags in tpblock.
    fn write_rt_flags_to_tpblock(&self, reg: GeneralRegister) {
        self.store(
            reg,
            GeneralRegister::Tp,
            self.rt_config.tp_block_rt_flags_offset(),
        );
    }

    // Clear out value of rt_flags in tpblock by writing zeros to it.
    fn clear_rt_flags_in_tpblock(&self) {
        self.comment("Clear out RT state (flags) in tpblock");
        self.write_rt_flags_to_tpblock(GeneralRegister::Zero);
    }

    // Read out rt_flags from tpblock into given register `reg`.
    fn read_rt_flags_from_tpblock(&self, reg: GeneralRegister) {
        self.load(
            reg,
            GeneralRegister::Tp,
            self.rt_config.tp_block_rt_flags_offset(),
        );
    }

    // Write value in given register `reg` for rt_flags to trapframe (assuming sp points to trapframe)
    fn store_rt_flags_to_trapframe(&self, reg: GeneralRegister) {
        self.store(
            reg,
            GeneralRegister::Sp,
            self.rt_config.rt_state_addr_offset(),
        );
    }

    // Read value of rt_flags in trapframe (assuming sp points to trapframe) to given register `reg`
    fn load_rt_flags_from_trapframe(&self, reg: GeneralRegister) {
        self.load(
            reg,
            GeneralRegister::Sp,
            self.rt_config.rt_state_addr_offset(),
        );
    }

    // Write value in given register `reg` for trap frame address to tpblock
    fn store_trap_frame_address_to_tpblock(&self, reg: GeneralRegister) {
        self.store(
            reg,
            GeneralRegister::Tp,
            self.rt_config.tp_block_trap_frame_offset(),
        );
    }

    // Read value of trap context frame address from tpblock to given register `reg`
    fn load_trap_frame_address_from_tpblock(&self, reg: GeneralRegister) {
        self.load(
            reg,
            GeneralRegister::Tp,
            self.rt_config.tp_block_trap_frame_offset(),
        );
    }
}

fn zero_trap_csrs(asm: &AsmBuilder) {
    asm.comment("Zero out interrupt/exception CSRs");
    asm.csrw_zero(Csr::Ie);
    if asm.rt_config.rv_mode() == RvMode::MMode {
        asm.csrw_zero(Csr::Mideleg);
        asm.csrw_zero(Csr::Medeleg);
    }
}

fn write_gp(asm: &AsmBuilder) {
    asm.comment("Set up global pointer");
    asm.option_push();
    asm.option_norelax();
    asm.la(GeneralRegister::Gp, "_global_pointer");
    asm.option_pop();
}

fn forward_label(label: &str) -> String {
    format!("{label:#}f")
}

fn backward_label(label: &str) -> String {
    format!("{label:#}b")
}

fn zero_bss(asm: &AsmBuilder) {
    if asm.rt_config.is_skip_bss_clearing() {
        return;
    }
    asm.comment("Zero out BSS");
    let start_reg = asm.get_free_reg();
    let end_reg = asm.get_free_reg();

    asm.la(start_reg, &SectionType::Bss.section_entry_start_symbol());
    asm.la(end_reg, &SectionType::Bss.section_entry_end_symbol());

    let loop_label = asm.next_label();
    let exit_label = asm.next_label();

    asm.bgeu(start_reg, end_reg, &forward_label(&exit_label));
    asm.label(&loop_label, None, None, None);
    asm.store_zero(start_reg);
    asm.addi(start_reg, start_reg, asm.rt_config.xlen_bytes());
    asm.bltu(start_reg, end_reg, &backward_label(&loop_label));
    asm.label(&exit_label, None, None, None);

    asm.release_reg(start_reg);
    asm.release_reg(end_reg);

    if asm.rt_config.is_multi_hart() {
        let addr_reg = asm.get_free_reg();
        let val_reg = asm.get_free_reg();

        asm.comment("Mark BSS init done");
        asm.la(addr_reg, &asm.get_label_from_map(LabelType::BssInitDone));
        asm.li_constrained(val_reg, 1);
        asm.store(val_reg, addr_reg, 0);

        asm.release_reg(addr_reg);
        asm.release_reg(val_reg);
    }
}

fn init_stack_pointer_using_boot_id(asm: &AsmBuilder) {
    asm.comment("Initialize stack pointer using boot id");

    let sub = asm.get_free_reg();
    asm.li_unconstrained(sub, asm.rt_config.hart_stack_size());
    asm.mul(sub, sub, asm.get_boot_id_reg());

    let sp = GeneralRegister::Sp;
    asm.la(sp, &stack_top_symbol());
    asm.sub(sp, sp, sub);

    asm.release_reg(sub);
}

fn handle_nonboot_harts(asm: &AsmBuilder) {
    let boot_hart_label = asm.next_label();
    let nonboot_addr_reg = asm.get_free_reg();

    asm.comment("Jump to non-boot hart handling");
    asm.beqz(asm.get_boot_id_reg(), &forward_label(&boot_hart_label));
    asm.la(
        nonboot_addr_reg,
        &asm.get_label_from_map(LabelType::SecondaryStart),
    );
    asm.jr(nonboot_addr_reg);
    asm.label(&boot_hart_label, None, None, None);
    asm.release_reg(nonboot_addr_reg);
}

fn protect_stack(asm: &AsmBuilder) {
    asm.comment("Place a sentry value at the bottom of the current hart's stack to try to detect future stack overflows");
    let stack_bottom = asm.get_free_reg();
    // assumption here: sp holds the top of the stack
    asm.mov(stack_bottom, GeneralRegister::Sp);
    let sub = asm.get_free_reg();
    asm.li_unconstrained(sub, asm.rt_config.hart_stack_size());
    asm.sub(stack_bottom, stack_bottom, sub);

    asm.release_reg(sub);

    let sentry_value = asm.get_free_reg();

    if asm.rt_config.target_config.hart_config.rv_xlen == RvXlen::Rv32 {
        asm.li_unconstrained(sentry_value, SENTRY_VALUE_RV32 as usize);
    } else {
        asm.li_unconstrained(sentry_value, SENTRY_VALUE_RV64);
    }
    asm.store(sentry_value, stack_bottom, 0);

    asm.release_reg(sentry_value);
    asm.release_reg(stack_bottom);
}

fn switch_to(asm: &AsmBuilder) {
    // Drain free reg pool. We don't have any free regs at this point.
    asm.drain_free_reg_pool();
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    asm.global_function(&GEN_FUNC_MAP.asm_fn(GeneratedFunc::SwitchTo));
    asm.comment("input: a0 contains address of the thread block to switch to");
    let sp = GeneralRegister::Sp;
    let ra = GeneralRegister::Ra;
    let tp = GeneralRegister::Tp;
    let a0 = GeneralRegister::A0;

    asm.comment("save interrupted registers first");
    asm.store(sp, tp, asm.rt_config.interrupted_mode_stack_offset());
    asm.store(tp, tp, asm.rt_config.interrupted_mode_tp_offset());

    asm.comment("We want to return back to ra, so set it as mepc");
    asm.csrw(Csr::Epc, ra);

    asm.comment("Write ra to tpblock.return_address so that it is saved correctly");
    asm.store(ra, tp, asm.rt_config.return_addr_offset());

    asm.comment("Set RT flag to indicate that trapframe address must be restored on switching back to this context");
    // Set up RT flags in `sp` which is stashed in tp block above
    asm.set_rt_flag_bit(sp, RtFlagBit::RestoreTrapFrameInTpBlock);
    // Write RT flags to tpblock so that they can be correctly updated in trapframe later
    asm.write_rt_flags_to_tpblock(sp);
    // Restore sp back from the stashed storage in tpblock.
    asm.load(sp, tp, asm.rt_config.interrupted_mode_stack_offset());

    let create_trap_frame_label = asm.get_label_from_map(LabelType::CreateTrapFrame);
    asm.comment("save current context now");
    asm.jal(&create_trap_frame_label);

    asm.init_default_free_reg_pool();
    let trap_reg = asm.get_free_reg();
    asm.comment("Save just created frame to priv mode context");
    asm.load(trap_reg, tp, asm.rt_config.context_addr_offset());
    asm.store(sp, trap_reg, asm.rt_config.priv_ctx_offset());

    asm.comment("Store priv mode context (passed in a0) as current context");
    asm.store(a0, tp, asm.rt_config.context_addr_offset());
    asm.comment("Zero out current mode sp in TpBlock since we are switching threads");
    asm.comment("this gets initialized on trap exit to lower mode and nested trap entry paths.");
    asm.store(
        GeneralRegister::Zero,
        tp,
        asm.rt_config.current_mode_stack_offset(),
    );
    asm.comment("Switch priv context to the one provided in a0");
    asm.load(sp, a0, asm.rt_config.priv_ctx_offset());
    asm.comment(
        "Zero out priv context frame address in context being switched to since we are restoring it now",
    );
    asm.store(GeneralRegister::Zero, a0, asm.rt_config.priv_ctx_offset());

    asm.comment("some task are hart agnostic. Make sure when they resume");
    asm.comment("they get to run with tp of the hart that invoked them");
    asm.store(tp, sp, asm.rt_config.tp_reg_offset());
    asm.j(&asm.get_label_from_map(LabelType::RestoreTrapFrame));
}

fn goto_rust_entrypoint(asm: &AsmBuilder) {
    asm.label(
        &asm.get_label_from_map(LabelType::JumpToRustEntrypoint),
        Some(RV_INSTRUCTION_ALIGNMENT_BYTES),
        Some(&text_default_section()),
        Some(asm.text_section_flags()),
    );
    let tp = GeneralRegister::Tp;
    let ra = GeneralRegister::Ra;
    asm.comment("save RA before we lose it due to jal");
    asm.store(ra, tp, asm.rt_config.return_addr_offset());

    let create_trap_frame_label = asm.get_label_from_map(LabelType::CreateTrapFrame);
    asm.jal(&create_trap_frame_label);

    // All general-purpose registers (except sp, tp) are stashed. So, initialize free reg pool
    asm.init_default_free_reg_pool();

    // Global pointer (GP) needs to be written before jumping to Rust environment. It is done here
    // after trap frame is created so that we don't corrupt the GP for the interrupted context.
    write_gp(asm);

    // Store trap frame address in tpblock. `sp` points to start of trap context frame.
    asm.comment("Store trap frame address (current sp value) in tpblock");
    asm.store_trap_frame_address_to_tpblock(GeneralRegister::Sp);

    let reg = asm.get_free_reg();
    let restore_trap_frame_label = asm.get_label_from_map(LabelType::RestoreTrapFrame);

    asm.comment(&format!(
        "On return from Rust, goto {:#}",
        &restore_trap_frame_label
    ));
    asm.load(reg, tp, asm.rt_config.rust_entrypoint_offset());
    asm.la(GeneralRegister::Ra, &restore_trap_frame_label);

    asm.jr(reg);
    asm.release_reg(reg);
}

fn jump_to_rust_entrypoint(asm: &AsmBuilder, entrypoint: &str) {
    write_entrypoint_in_tp(asm, entrypoint);
    if asm.rt_config.needs_stack_overflow_detection() {
        asm.j(&asm.get_label_from_map(LabelType::ProtectStack));
    } else {
        asm.j(&asm.get_label_from_map(LabelType::JumpToRustEntrypoint));
    }
}

fn protect_stack_section(asm: &AsmBuilder) {
    asm.label(
        &asm.get_label_from_map(LabelType::ProtectStack),
        Some(RV_INSTRUCTION_ALIGNMENT_BYTES),
        Some(&text_default_section()),
        Some(asm.text_section_flags()),
    );
    protect_stack(asm);
    asm.j(&asm.get_label_from_map(LabelType::JumpToRustEntrypoint));
}

fn nonboot_hart_call_rust_entrypoint(asm: &AsmBuilder) {
    asm.label(
        &asm.get_label_from_map(LabelType::SecondaryStart),
        Some(RV_INSTRUCTION_ALIGNMENT_BYTES),
        None,
        None,
    );
    wait_for_bss_init_done(asm);
    asm.comment("Jump to Rust entrypoint on non-boot hart");
    jump_to_rust_entrypoint(asm, asm.rt_config.nonboot_hart_rust_entrypoint());
}

fn boothart_call_rust_entrypoint(asm: &AsmBuilder) {
    asm.comment("Jump to Rust entrypoint on boot hart");
    jump_to_rust_entrypoint(asm, asm.rt_config.boot_hart_rust_entrypoint());
}

fn park_hart(asm: &AsmBuilder) {
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    let park_label = asm.get_label_from_map(LabelType::ParkHart);
    asm.global_function(&park_label);
    asm.wfi();
    asm.j(&park_label);
}

fn define_hart_idx_variable(asm: &AsmBuilder) {
    asm.label(
        &asm.get_label_from_map(LabelType::BootIdxVariable),
        None,
        Some(&data_default_section()),
        None,
    );
    asm.comment("Variable for determining boot id");
    asm.xword(0);
    asm.end_section();
}

// Defining a default thread pointer block. This can be used by projects that don't care about
// maintaining multiple contexts and stacks in the current mode. For cases where this is not
// true - example S-mode kernel wanting to store a separate stack per task, this thread
// pointer block can be defined differently by using some flag
fn define_thread_pointer_block(asm: &AsmBuilder) {
    asm.label(
        &asm.get_label_from_map(LabelType::ThreadPointerBlock),
        None,
        Some(&data_default_section()),
        None,
    );
    asm.comment("Thread pointer block storage");
    asm.rept(
        asm.rt_config.max_hart_count() * asm.rt_config.tp_block_size() as usize,
        0,
    );
    asm.end_section();
}

fn define_bss_init_done(asm: &AsmBuilder) {
    if asm.rt_config.is_skip_bss_clearing() {
        return;
    }
    asm.label(
        &asm.get_label_from_map(LabelType::BssInitDone),
        None,
        Some(&data_default_section()),
        None,
    );
    asm.comment("Variable for indicating bss clearing status");
    asm.xword(0);
    asm.end_section();
}

fn wait_for_bss_init_done(asm: &AsmBuilder) {
    if asm.rt_config.is_skip_bss_clearing() {
        return;
    }
    let addr_reg = asm.get_free_reg();
    let val_reg = asm.get_free_reg();

    let loopback_label = asm.next_label();
    asm.comment("Wait for BSS init done");
    asm.la(addr_reg, &asm.get_label_from_map(LabelType::BssInitDone));
    asm.label(&loopback_label, None, None, None);
    asm.load(val_reg, addr_reg, 0);
    asm.beqz(val_reg, &backward_label(&loopback_label));

    asm.release_reg(addr_reg);
    asm.release_reg(val_reg);
}

fn hart_count_error_handling(asm: &AsmBuilder) {
    let max_hart_count = asm.get_free_reg();
    let boot_label = asm.next_label();
    let park_addr_reg = asm.get_free_reg();

    asm.comment("Park hart if boot id is greater than max hart count defined in configuration");
    asm.li_constrained(max_hart_count, asm.rt_config.max_hart_count());
    asm.bltu(
        asm.get_boot_id_reg(),
        max_hart_count,
        &forward_label(&boot_label),
    );
    asm.la(park_addr_reg, &asm.get_label_from_map(LabelType::ParkHart));
    asm.jr(park_addr_reg);
    asm.label(&boot_label, None, None, None);
    asm.release_reg(max_hart_count);
    asm.release_reg(park_addr_reg);
}

fn read_hart_id(asm: &AsmBuilder) {
    let hart_id = asm.get_hart_id_reg();

    asm.comment("Read hart id");
    // Assumption is that hart ID can be read from mhartid when in M-mode
    // and will be passed in A0 by previous component for S-mode.
    match asm.rt_config.rv_mode() {
        RvMode::MMode => asm.csrr(hart_id, Csr::Mhartid),
        RvMode::SMode => asm.mov(hart_id, GeneralRegister::A0),
    }
}

fn determine_boot_id(asm: &AsmBuilder) {
    let boot_id = asm.get_boot_id_reg();

    if asm.rt_config.is_multi_hart() {
        asm.comment("Determine boot id");
        asm.la(boot_id, &asm.get_label_from_map(LabelType::BootIdxVariable));

        let inc = asm.get_free_reg();
        asm.li_constrained(inc, 1);

        // Assumption is that hart supports AMOADD in case of multi-hart configuration
        // This is for assigning boot id.
        asm.amoadd(boot_id, boot_id, inc);
        asm.release_reg(inc);

        hart_count_error_handling(asm);
    } else {
        // For single-hart configurations, assume boot id as 0
        asm.mov(boot_id, GeneralRegister::Zero);
    }
}

fn get_stack_bottom(stack_bottom_reg: GeneralRegister, asm: &AsmBuilder) {
    asm.comment("Get stack bottom using boot id");

    let sub = asm.get_free_reg();
    asm.li_unconstrained(sub, asm.rt_config.hart_stack_size());
    let offset = asm.get_free_reg();
    // We should not get boot_id_reg using asm.get_boot_id_reg() as it's been
    // released at this point.
    let boot_id_reg = asm.get_free_reg();
    asm.load(
        boot_id_reg,
        GeneralRegister::Tp,
        asm.rt_config.boot_id_offset(),
    );
    asm.addi(offset, boot_id_reg, 1);
    asm.mul(sub, sub, offset);
    asm.release_reg(boot_id_reg);
    asm.release_reg(offset);

    asm.la(stack_bottom_reg, &stack_top_symbol());
    asm.sub(stack_bottom_reg, stack_bottom_reg, sub);
    asm.release_reg(sub);
}

fn check_stack(asm: &AsmBuilder) {
    asm.comment("Perform stack overflow detection");

    let stack_bottom_reg = asm.get_free_reg();
    get_stack_bottom(stack_bottom_reg, asm);

    let value_reg = asm.get_free_reg();
    asm.load(value_reg, stack_bottom_reg, 0);

    let sentry_value = asm.get_free_reg();
    if asm.rt_config.target_config.hart_config.rv_xlen == RvXlen::Rv32 {
        asm.li_unconstrained(sentry_value, SENTRY_VALUE_RV32 as usize);
    } else {
        asm.li_unconstrained(sentry_value, SENTRY_VALUE_RV64);
    }

    let next_label = asm.next_label();
    asm.comment("If stack overflow is detected, jump to stack overflow handler");

    asm.beq(value_reg, sentry_value, &forward_label(&next_label));

    let rs = asm.get_free_reg();
    asm.la(rs, asm.rt_config.stack_overflow_handle_entrypoint());
    asm.comment("we are returning to park hart as this indicates something went wrong and we cannot recover from this");
    asm.la(
        GeneralRegister::Ra,
        &asm.get_label_from_map(LabelType::ParkHart),
    );

    asm.comment("Expected value in a0");
    asm.mov(GeneralRegister::A0, sentry_value);
    asm.comment("Actual current value in a1");
    asm.mov(GeneralRegister::A1, value_reg);
    asm.jr(rs);
    asm.release_reg(rs);

    asm.label(&next_label, None, None, None);

    asm.release_reg(stack_bottom_reg);
    asm.release_reg(value_reg);
    asm.release_reg(sentry_value);
}

fn align_up(val: usize, align_to: usize) -> usize {
    assert!(align_to.is_power_of_two(), "Alignment must be a power of 2");
    (val + align_to - 1) & !(align_to - 1)
}

fn aligned_trap_frame_size(trap_frame_size: usize) -> usize {
    align_up(trap_frame_size, 16)
}

fn restore_trap_frame(asm: &AsmBuilder) {
    let sp = GeneralRegister::Sp;
    let tp = GeneralRegister::Tp;
    let reg_size = asm.rt_config.xlen_bytes();

    asm.label(
        &asm.get_label_from_map(LabelType::RestoreTrapFrame),
        Some(RV_INSTRUCTION_ALIGNMENT_BYTES),
        Some(&text_default_section()),
        Some(asm.text_section_flags()),
    );

    if asm.rt_config.needs_stack_overflow_detection() {
        check_stack(asm);
    }

    // Unwind current mode stack if returning to lower privilege mode
    let pp = asm.get_free_reg();
    let status = asm.get_free_reg();
    let restore_label = asm.next_label();

    asm.comment("Check if returning to lower privilege mode");
    asm.load(status, sp, asm.rt_config.status_reg_offset());
    // pp bits are shifted into place as the bitfields themselves and the value
    // can be either 6144 or 256 in decimal. So we are using li_unconstrained()
    // here
    asm.li_unconstrained(pp, asm.rt_config.rv_mode().as_pp());
    asm.and(status, status, pp);
    asm.beq(status, pp, &forward_label(&restore_label));

    asm.release_reg(pp);
    asm.release_reg(status);

    let temp_reg = asm.get_free_reg();
    asm.comment(
        "Save unwound stack pointer in thread block structure if returning to lower privilege mode",
    );
    let total_size = aligned_trap_frame_size(asm.rt_config.trap_frame_size() as usize);
    let comment = format!(
        "The size = {}: size of trap frame {} being aligned up to 16 bytes since we aligned sp down to be 16-byte aligned in jump_to_rust",
        total_size, asm.rt_config.trap_frame_size()
    );
    asm.comment(comment.as_str());
    asm.addi(temp_reg, sp, total_size as isize);
    asm.store(temp_reg, tp, asm.rt_config.current_mode_stack_offset());

    asm.csrw(Csr::Scratch, tp);

    asm.label(&restore_label, None, None, None);
    let restore_csr_label = asm.next_label();

    // Restore trapframe address only if rt_flags say so.
    asm.comment(&format!(
        "Restore previous trapframe address to thread pointer block if rt_flags say so (bit {})",
        RtFlagBit::RestoreTrapFrameInTpBlock as u8
    ));
    asm.load_rt_flags_from_trapframe(temp_reg);
    asm.andi(
        temp_reg,
        temp_reg,
        RtFlagBit::RestoreTrapFrameInTpBlock.as_mask(),
    );
    asm.beqz(temp_reg, &forward_label(&restore_csr_label));

    asm.load(temp_reg, sp, asm.rt_config.interrupted_frame_addr_offset());
    asm.store_trap_frame_address_to_tpblock(temp_reg);

    if asm.rt_config.sfence_on_trapframe_restore_feature {
        asm.load_rt_flags_from_trapframe(temp_reg);
        let no_sfence = asm.next_label();
        asm.andi(
            temp_reg,
            temp_reg,
            RtFlagBit::TranslationRegChanged.as_mask(),
        );
        asm.beqz(temp_reg, &forward_label(&no_sfence));

        asm.sfence(GeneralRegister::Zero, GeneralRegister::Zero);

        asm.label(&no_sfence, None, None, None);
    }

    // First restore the floating point registers
    if asm.rt_config.floating_point_support {
        asm.comment("Now restore floating point registers if required");
        let fs_clean = asm.next_label();

        asm.load_rt_flags_from_trapframe(temp_reg);
        asm.andi(temp_reg, temp_reg, RtFlagBit::FsStateWasDirty.as_mask());
        asm.beqz(temp_reg, &forward_label(&fs_clean));

        let fr_start_idx = asm.rt_config.trap_frame.fr_start_idx();
        for (idx, fr) in asm
            .rt_config
            .trap_frame
            .floating_point_registers
            .iter()
            .enumerate()
        {
            let offset = (idx as isize + fr_start_idx) * reg_size;
            asm.fload(*fr, sp, offset);
        }

        // The state is now clean
        asm.load_rt_flags_from_trapframe(temp_reg);
        asm.andi(temp_reg, temp_reg, !RtFlagBit::FsStateWasDirty.as_mask());
        asm.store_rt_flags_to_trapframe(temp_reg);

        asm.label(&fs_clean, None, None, None);
    }

    // Now restore the CSRs using general registers and then restore general registers.
    asm.label(&restore_csr_label, None, None, None);
    asm.comment("Restore all CSRs first since they require a general register for csrw");
    let csr_start_idx = asm.rt_config.trap_frame.csr_start_idx();
    for (idx, csr) in asm.rt_config.trap_frame.csrs.iter().enumerate() {
        if csr.restore_from_trap_frame() {
            asm.load(temp_reg, sp, (idx as isize + csr_start_idx) * reg_size);
            asm.csrw(*csr, temp_reg);
        }
    }

    asm.release_reg(temp_reg);

    asm.comment("Now restore all general registers except sp - sp is restored last");
    let gr_start_idx = asm.rt_config.trap_frame.gr_start_idx();
    for (idx, gr) in asm.rt_config.trap_frame.general_regs.iter().enumerate() {
        if *gr == sp {
            // SP is restored just before performing ret
            assert!(idx != 0, "sp is at idx 0");
            continue;
        }

        let offset = (idx as isize + gr_start_idx) * reg_size;
        asm.load(*gr, sp, offset);

        if asm.rt_config.supports_atomic_extension() && idx == 0 {
            asm.comment("Clear any reservations before performing a context switch");
            asm.sc(GeneralRegister::Zero, *gr, sp);
        }
    }

    asm.comment("Restore sp and perform return from mode");
    asm.load(sp, sp, asm.rt_config.sp_reg_offset());
    asm.mode_ret();
}

fn write_epc(asm: &AsmBuilder) {
    // Configure EPC to point to _park_hart so that a return to assembly code
    // back from the hart rust entrypoint results in hart going into wfi loop.
    let reg = asm.get_free_reg();
    asm.comment("Default action is to park hart on return from Rust code, unless epc is changed by the called code");
    asm.la(reg, &asm.get_label_from_map(LabelType::ParkHart));
    asm.csrw(Csr::Epc, reg);
    asm.release_reg(reg);
}

fn write_status(asm: &AsmBuilder) {
    let reg = asm.get_free_reg();
    asm.comment("Default action is to return back to current mode on return from Rust code, unless changed by called code");
    // pp bits are shifted into place as the bitfields themselves and the value
    // can be either 6144 or 256 in decimal. So we are using li_unconstrained()
    // here
    asm.li_unconstrained(reg, asm.rt_config.rv_mode().as_mask());
    asm.csrc(Csr::Status, reg);

    asm.li_unconstrained(reg, asm.rt_config.rv_mode().as_pp());
    asm.csrs(Csr::Status, reg);
    asm.release_reg(reg);
}

fn text_reset_section(asm: &AsmBuilder) {
    asm.global_entrypoint(&reset_section());
}

fn call_custom_reset_entrypoint(asm: &AsmBuilder) {
    let rs = asm.get_free_reg();
    let comment = format!(
        "The component that uses this lib needs to provide '{}' in its own .S file",
        asm.rt_config.custom_reset_entrypoint()
    );
    asm.comment(comment.as_str());
    asm.la(rs, asm.rt_config.custom_reset_entrypoint());
    asm.jalr(GeneralRegister::Ra, rs, 0);
    asm.release_reg(rs);
}

fn create_trap_frame(asm: &AsmBuilder) {
    let sp = GeneralRegister::Sp;
    let tp = GeneralRegister::Tp;
    let ra = GeneralRegister::Ra;
    let scratch = Csr::Scratch;
    let reg_size = asm.rt_config.xlen_bytes();
    asm.comment("Create new trapframe");
    asm.label(
        &asm.get_label_from_map(LabelType::CreateTrapFrame),
        Some(RV_INSTRUCTION_ALIGNMENT_BYTES),
        Some(&text_default_section()),
        Some(asm.text_section_flags()),
    );
    asm.addi(sp, sp, -asm.rt_config.trap_frame_size());

    asm.comment("Align sp down to ensure it is 16-byte aligned by performing andi sp, sp, ~0xf. This is required by the spec");
    asm.comment("We are doing this in two steps with the following andi instruction(instead of sub the aligned size directly)");
    asm.comment("since in case of nested trap, sp can not be guaranteed to be aligned upon entry.");

    asm.andi(sp, sp, -16);

    // First stash the general registers(except SP, TP and RA). Stashed general registers can then be used to read CSRs.
    // SP and TP are saved later since these are stashed from elsewhere: SP <- thread pointer block, TP <- scratch register
    asm.comment("First stash away all the general registers in trap frame except SP, TP and RA - those are stashed from elsewhere");
    let gr_start_idx = asm.rt_config.trap_frame.gr_start_idx();
    for (idx, gr) in asm.rt_config.trap_frame.general_regs.iter().enumerate() {
        if *gr != sp && *gr != tp && *gr != ra {
            asm.store(*gr, sp, (idx as isize + gr_start_idx) * reg_size);
        }
    }

    // All general-purpose registers (except sp, tp) are stashed. So, initialize free reg pool
    asm.init_default_free_reg_pool();

    // Save floating point registers if required
    if asm.rt_config.floating_point_support {
        asm.comment("Check if FS is dirty and if so, stash the floating-point registers");
        let fs_clean = asm.next_label();

        let status_reg = asm.get_free_reg();
        let temp_reg = asm.get_free_reg();
        let mask_reg = asm.get_free_reg();

        // Check for FS != Dirty
        asm.csrr(status_reg, Csr::Status);
        asm.li_unconstrained(mask_reg, STATUS_FS_MASK_DIRTY);
        asm.and(temp_reg, status_reg, mask_reg);
        asm.bne(temp_reg, mask_reg, &forward_label(&fs_clean));

        // It is dirty, so stash the FP registers
        let fr_start_idx = asm.rt_config.trap_frame.fr_start_idx();
        for (idx, fr) in asm
            .rt_config
            .trap_frame
            .floating_point_registers
            .iter()
            .enumerate()
        {
            asm.fstore(*fr, sp, (idx as isize + fr_start_idx) * reg_size);
        }

        // Set FS state to Clean
        asm.comment("Now that the FP registers are stashed, set the FS state to Clean");
        // Invert the mask
        asm.xori(mask_reg, mask_reg, -1);
        // Clear the FS bits
        asm.and(temp_reg, mask_reg, status_reg);
        // Write Clean state into FS
        asm.li_unconstrained(mask_reg, STATUS_FS_CLEAN);
        asm.or(status_reg, temp_reg, mask_reg);
        asm.csrw(Csr::Status, status_reg);
        asm.release_reg(status_reg);

        // Indicate that the floating point state needs to be restored as well
        asm.comment("Record the fact that the FP registers will need to be restored in RT flags");
        asm.read_rt_flags_from_tpblock(temp_reg);
        asm.li_unconstrained(
            mask_reg,
            RtFlagBit::FsStateWasDirty.as_mask().try_into().unwrap(),
        );
        asm.or(temp_reg, temp_reg, mask_reg);
        asm.write_rt_flags_to_tpblock(temp_reg);

        asm.release_reg(mask_reg);
        asm.release_reg(temp_reg);

        asm.label(&fs_clean, None, None, None);
    }

    let temp_reg = asm.get_free_reg();

    // Stash SP from thread pointer block
    asm.comment(
        "Stash SP in trap frame using the interrupted mode stack value in thread pointer block",
    );
    asm.load(temp_reg, tp, asm.rt_config.interrupted_mode_stack_offset());
    asm.store(temp_reg, sp, asm.rt_config.sp_reg_offset());

    asm.comment("get ra from thread pointer block and save");
    asm.load(temp_reg, tp, asm.rt_config.return_addr_offset());
    asm.store(temp_reg, sp, asm.rt_config.ra_reg_offset());

    // Stash TP from scratch register
    asm.comment("Stash TP in trap frame using the scratch register value");
    asm.load(temp_reg, tp, asm.rt_config.interrupted_mode_tp_offset());
    asm.store(temp_reg, sp, asm.rt_config.tp_reg_offset());

    // Write 0 to scratch register so that nested traps know that we were already in current mode
    asm.comment("Write 0 to scratch register so that trap entry path knows if we encounter a nested trap in current mode");
    asm.csrw(scratch, GeneralRegister::Zero);

    asm.comment("Stash all the CSRs in trap frame");
    let csr_start_idx = asm.rt_config.trap_frame.csr_start_idx();
    for (idx, csr) in asm.rt_config.trap_frame.csrs.iter().enumerate() {
        asm.csrr(temp_reg, *csr);
        asm.store(temp_reg, sp, (idx as isize + csr_start_idx) * reg_size);
    }

    // Store rt flags from thread pointer block to trapframe and zero-out flags from thread pointer block
    asm.comment("Read RT state (flags) from tpblock and save to trapframe");
    asm.read_rt_flags_from_tpblock(temp_reg);
    asm.store_rt_flags_to_trapframe(temp_reg);
    asm.clear_rt_flags_in_tpblock();

    // Stash trap context frame from thread pointer block
    asm.comment("Stash trap ctx frame address in current trapframe");
    asm.load_trap_frame_address_from_tpblock(temp_reg);
    asm.store(temp_reg, sp, asm.rt_config.interrupted_frame_addr_offset());

    asm.release_reg(temp_reg);
    asm.ret();
}

fn handle_trap(asm: &AsmBuilder) {
    let sp = GeneralRegister::Sp;
    let tp = GeneralRegister::Tp;
    let scratch = Csr::Scratch;

    let not_nested_label = asm.next_label();
    let jump_ahead_label = asm.next_label();

    asm.label(
        &asm.get_label_from_map(LabelType::HandleTrap),
        Some(RV_INSTRUCTION_ALIGNMENT_BYTES),
        Some(&text_default_section()),
        Some(asm.text_section_flags()),
    );
    asm.comment("Check if this is a nested trap. If yes, then scratch would be 0");
    asm.csrrw(tp, scratch, tp);
    asm.bnez(tp, &forward_label(&not_nested_label));
    asm.comment("For nested trap, read back tp from scratch");
    asm.csrr(tp, scratch);
    asm.comment("Store current stack pointer as current mode stack to use");
    asm.store(sp, tp, asm.rt_config.current_mode_stack_offset());
    asm.comment("Set rt state(flags) to indicate we are in nested mode. No free reg to use. So, let's use sp and restore it back from tpblock.");
    // Set up RT flags in `sp` which is the only free register to use
    asm.set_rt_flag_bit(sp, RtFlagBit::RestoreTrapFrameInTpBlock);
    // Write RT flags to tpblock so that they can be correctly updated in trapframe later
    asm.write_rt_flags_to_tpblock(sp);
    // Restore sp back from the stashed storage in tpblock.
    asm.load(sp, tp, asm.rt_config.current_mode_stack_offset());
    asm.j(&forward_label(&jump_ahead_label));

    asm.label(&not_nested_label, None, None, None);
    asm.comment("Not in recursive trap. Clear out rt flags in tp block");
    asm.clear_rt_flags_in_tpblock();

    asm.label(&jump_ahead_label, None, None, None);
    asm.comment(
        "Store current stack pointer as interrupted mode stack pointer to restore on return path",
    );
    asm.store(sp, tp, asm.rt_config.interrupted_mode_stack_offset());

    // At this point, we have SP stashed away so it can be used as free reg
    asm.assign_free_reg_pool(&[sp]);

    let reg = asm.get_free_reg();
    asm.csrr(reg, scratch);
    asm.store(reg, tp, asm.rt_config.interrupted_mode_tp_offset());
    asm.release_reg(reg);

    asm.comment("We only have SP register available to use as temp reg to stash Rust entrypoint");
    write_entrypoint_in_tp(asm, asm.rt_config.trap_rust_entrypoint());

    // We will be using SP now, so don't treat it as a free reg anymore
    asm.drain_free_reg_pool();

    asm.comment("Load current mode stack pointer to start using stack in current mode");
    asm.load(sp, tp, asm.rt_config.current_mode_stack_offset());

    asm.j(&asm.get_label_from_map(LabelType::JumpToRustEntrypoint));
}

fn write_scratch(asm: &AsmBuilder) {
    let tp = GeneralRegister::Tp;
    asm.comment("Initialize scratch pointer with thread pointer block storage to make the return path same as trap return");
    asm.la(tp, &asm.get_label_from_map(LabelType::ThreadPointerBlock));

    let reg = asm.get_free_reg();
    asm.li_constrained(reg, asm.rt_config.tp_block_size() as usize);
    asm.mul(reg, reg, asm.get_boot_id_reg());
    asm.add(tp, tp, reg);
    asm.release_reg(reg);
    asm.store(asm.get_boot_id_reg(), tp, asm.rt_config.boot_id_offset());
    asm.store(asm.get_hart_id_reg(), tp, asm.rt_config.hart_id_offset());

    asm.csrw(Csr::Scratch, tp);
}

fn write_sptp(asm: &AsmBuilder) {
    let sp = GeneralRegister::Sp;
    let tp = GeneralRegister::Tp;
    asm.comment("Store current stack pointer as interrupted and current mode stack pointer in thread pointer block to make return path same as trap return");
    asm.store(sp, tp, asm.rt_config.interrupted_mode_stack_offset());
    asm.store(sp, tp, asm.rt_config.current_mode_stack_offset());
}

fn write_init_rtflags(asm: &AsmBuilder) {
    // Clear out RT flags in tpblock for the init path
    asm.clear_rt_flags_in_tpblock();
}

fn write_entrypoint_in_tp(asm: &AsmBuilder, entrypoint: &str) {
    let reg = asm.get_free_reg();
    let tp = GeneralRegister::Tp;

    asm.comment("Write out the Rust entrypoint address in thread pointer block");
    asm.la(reg, entrypoint);
    asm.store(reg, tp, asm.rt_config.rust_entrypoint_offset());

    asm.release_reg(reg);
}

fn write_tvec(asm: &AsmBuilder) {
    let reg = asm.get_free_reg();
    asm.comment("Initialize trap vector base address");
    asm.la(reg, &asm.get_label_from_map(LabelType::HandleTrap));
    asm.csrw(Csr::Tvec, reg);
    asm.release_reg(reg);
}

fn init_fp(asm: &AsmBuilder) {
    let status_reg = asm.get_free_reg();
    let mask_reg = asm.get_free_reg();
    asm.comment("Set FS to Clean");
    asm.csrr(status_reg, Csr::Status);
    asm.li_unconstrained(mask_reg, !STATUS_FS_MASK_DIRTY);
    asm.and(status_reg, status_reg, mask_reg);
    asm.li_unconstrained(mask_reg, STATUS_FS_CLEAN);
    asm.or(status_reg, status_reg, mask_reg);
    asm.csrw(Csr::Status, status_reg);

    asm.comment("Clear FCSR");
    asm.csrw(Csr::Fcsr, GeneralRegister::Zero);

    asm.comment("Zero the FP registers");
    for fr in asm.rt_config.trap_frame.floating_point_registers.iter() {
        asm.move_to_float(*fr, GeneralRegister::Zero);
    }

    asm.release_reg(status_reg);
    asm.release_reg(mask_reg);
}

fn common_hart_init(asm: &AsmBuilder) {
    if asm.rt_config.target_config.needs_custom_reset() {
        call_custom_reset_entrypoint(asm);
    }

    determine_boot_id(asm);
    read_hart_id(asm);
    init_stack_pointer_using_boot_id(asm);
    zero_trap_csrs(asm);
    write_epc(asm);
    write_status(asm);
    write_tvec(asm);
    write_scratch(asm);
    write_sptp(asm);
    write_init_rtflags(asm);

    if asm.rt_config.floating_point_support {
        init_fp(asm);
    }
}

fn build_multi_hart_start(asm: &AsmBuilder) {
    text_reset_section(asm);

    common_hart_init(asm);

    // Jump to secondary label for non-boot harts
    handle_nonboot_harts(asm);

    // Only boot hart performs this initialization
    zero_bss(asm);
    boothart_call_rust_entrypoint(asm);

    // Secondary label for non-boot hart
    nonboot_hart_call_rust_entrypoint(asm);
}

fn build_boot_hart_start(asm: &AsmBuilder) {
    text_reset_section(asm);
    common_hart_init(asm);
    zero_bss(asm);
    boothart_call_rust_entrypoint(asm);
}

fn build_secondary_hart_start(asm: &AsmBuilder) {
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    asm.global_function(&asm.get_label_from_map(LabelType::SecondaryStart));
    common_hart_init(asm);
    wait_for_bss_init_done(asm);
    jump_to_rust_entrypoint(asm, asm.rt_config.nonboot_hart_rust_entrypoint());
}

fn asm_tp_block_base(asm: &AsmBuilder) {
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    asm.comment("Function to be called from non-assembly code");
    asm.global_function(&GEN_FUNC_MAP.asm_fn(GeneratedFunc::TpBlockBase));
    asm.comment("Load address of tp block in a0 as return value");
    asm.la(
        GeneralRegister::A0,
        &asm.get_label_from_map(LabelType::ThreadPointerBlock),
    );
    asm.comment("Return back to address in ra");
    asm.jr(GeneralRegister::Ra);
}

fn asm_get_rest_tf_label(asm: &AsmBuilder) {
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    asm.comment("Function to be called from non-assembly code");
    asm.global_function(&GEN_FUNC_MAP.asm_fn(GeneratedFunc::RestoreTrapFrame));
    asm.comment("Load address of rest tf in a0 as return value");
    asm.la(
        GeneralRegister::A0,
        &asm.get_label_from_map(LabelType::RestoreTrapFrame),
    );
    asm.comment("Return back to address in ra");
    asm.jr(GeneralRegister::Ra);
}

fn generate_asm_id(asm: &AsmBuilder, asm_fn_name: &str, tp_block_offset: isize) {
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    asm.comment("Function to be called from non-assembly code");
    asm.global_function(asm_fn_name);
    asm.comment("Take id from tp block and place it in a0 as return value");
    asm.load(GeneralRegister::A0, GeneralRegister::Tp, tp_block_offset);
    asm.comment("Return back to address in ra");
    asm.jr(GeneralRegister::Ra);
}

fn asm_my_ids(asm: &AsmBuilder) {
    generate_asm_id(
        asm,
        &GEN_FUNC_MAP.asm_fn(GeneratedFunc::BootId),
        asm.rt_config.boot_id_offset(),
    );
    generate_asm_id(
        asm,
        &GEN_FUNC_MAP.asm_fn(GeneratedFunc::HartId),
        asm.rt_config.hart_id_offset(),
    );
}

fn asm_my_trap_frame_addr(asm: &AsmBuilder) {
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    asm.comment("Function to be called from non-assembly code");
    asm.global_function(&asm.get_label_from_map(LabelType::GetTrapAddr));
    asm.comment("Take trap frame addr from tp block and place it in a0 as return value");
    asm.load_trap_frame_address_from_tpblock(GeneralRegister::A0);
    asm.comment("Return back to address in ra");
    asm.jr(GeneralRegister::Ra);
}

fn asm_my_tp_block_addr(asm: &AsmBuilder) {
    asm.align(RV_INSTRUCTION_ALIGNMENT_BYTES);
    asm.comment("Function to be called from non-assembly code");
    asm.global_function(&GEN_FUNC_MAP.asm_fn(GeneratedFunc::TpBlockAddr));
    asm.comment("Take tp block address from tp and place it in a0 as return value");
    asm.mov(GeneralRegister::A0, GeneralRegister::Tp);
    asm.comment("Return back to address in ra");
    asm.jr(GeneralRegister::Ra);
}

fn generate_rust_id(rust: &RustBuilder, rust_fn_name: String, asm_fn_name: String) {
    rust.new_c_extern();
    rust.func_prototype(asm_fn_name.clone(), Vec::new(), Some("usize".to_string()));
    rust.end_extern();

    rust.new_func_with_ret(rust_fn_name, "usize".to_string());
    rust.new_unsafe_block();
    rust.call_with_ret(asm_fn_name, Vec::new());
    rust.end_unsafe_block();
    rust.end_func();
}

fn rust_my_ids(rust: &RustBuilder) {
    generate_rust_id(
        rust,
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::BootId),
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::BootId),
    );
    generate_rust_id(
        rust,
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::HartId),
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::HartId),
    );
}

fn rust_my_trap_frame_addr(rust: &RustBuilder) {
    rust.new_c_extern();
    rust.func_prototype(
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::TrapFrameAddr),
        Vec::new(),
        Some("usize".to_string()),
    );
    rust.end_extern();

    rust.new_func_with_ret(
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::TrapFrameAddr),
        "usize".to_string(),
    );
    rust.new_unsafe_block();
    rust.call_with_ret(
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::TrapFrameAddr),
        Vec::new(),
    );
    rust.end_unsafe_block();
    rust.end_func();
}

fn rust_my_tp_block_addr(rust: &RustBuilder) {
    rust.new_c_extern();
    rust.func_prototype(
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::TpBlockAddr),
        Vec::new(),
        Some("usize".to_string()),
    );
    rust.end_extern();

    rust.new_func_with_ret(
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::TpBlockAddr),
        "usize".to_string(),
    );
    rust.new_unsafe_block();
    rust.call_with_ret(GEN_FUNC_MAP.asm_fn(GeneratedFunc::TpBlockAddr), Vec::new());
    rust.end_unsafe_block();
    rust.end_func();
}

fn rust_tp_block_mut(rust: &RustBuilder, rt_config: &RtConfig) {
    rust.new_func_with_ret(
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::TpBlock),
        format!("&'static mut {:#}", rt_config.tp_block.rust_struct_name()),
    );
    rust.new_unsafe_block();
    rust.implicit_ret(format!(
        "&mut *({:#}() as *mut {:#})",
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::TpBlockAddr),
        rt_config.tp_block.rust_struct_name()
    ));
    rust.end_unsafe_block();
    rust.end_func();
}

fn rust_get_rest_tf_label(rust: &RustBuilder) {
    rust.new_c_extern();
    rust.func_prototype(
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::RestoreTrapFrame),
        Vec::new(),
        Some("usize".to_string()),
    );
    rust.end_extern();

    rust.new_func_with_ret(
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::RestoreTrapFrame),
        "usize".to_string(),
    );
    rust.new_unsafe_block();
    rust.call_with_ret(
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::RestoreTrapFrame),
        Vec::new(),
    );
    rust.end_unsafe_block();
    rust.end_func();
}

fn rust_switch_to(rust: &RustBuilder, arg_name: String) {
    let prot_arg = arg_name.clone() + ": usize";
    let vpstr = vec![prot_arg.clone()];
    let vstr = vec![arg_name.clone()];
    rust.new_c_extern();
    rust.func_prototype(
        GEN_FUNC_MAP.asm_fn(GeneratedFunc::SwitchTo),
        vpstr.clone(),
        None,
    );
    rust.end_extern();

    rust.new_func_with_arg(
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::SwitchTo),
        vpstr[0].clone(),
    );
    rust.new_unsafe_block();
    rust.call_without_ret(GEN_FUNC_MAP.asm_fn(GeneratedFunc::SwitchTo), vstr);
    rust.end_unsafe_block();
    rust.end_func();
}

fn write_asm_helpers(asm: &AsmBuilder) {
    asm_my_ids(asm);
    asm_my_trap_frame_addr(asm);
    asm_my_tp_block_addr(asm);
    asm_tp_block_base(asm);
    asm_get_rest_tf_label(asm);
    switch_to(asm);
}

fn write_boot_s_file(dirpath: &Path, rt_config: &RtConfig, filename: &str) -> std::io::Result<()> {
    let filepath = dirpath.join(filename);
    let fw = FileWriter::new(filepath, BlockDelimiter::None);
    let asm = AsmBuilder::new(rt_config);

    asm.preamble();

    asm.add_labels(&[
        (LabelType::ResetStart, START_SYMBOL),
        (LabelType::ParkHart, "_park_hart"),
        (LabelType::SecondaryStart, "_secondary_start"),
        (LabelType::RestoreTrapFrame, "restore_trap_frame"),
        (LabelType::CreateTrapFrame, "create_trap_frame"),
        (LabelType::HandleTrap, "handle_trap"),
        (LabelType::JumpToRustEntrypoint, "jump_to_rust"),
        (LabelType::BootIdxVariable, "boot_idx"),
        (LabelType::ThreadPointerBlock, "tp_block"),
        (LabelType::BssInitDone, "bss_init_done"),
        (LabelType::ProtectStack, "protect_stack"),
        (LabelType::GetTrapAddr, "__my_trap_frame_addr"),
    ]);

    asm.init_default_free_reg_pool();

    asm.allocate_id_regs();

    if asm.rt_config.is_multi_hart() {
        define_hart_idx_variable(&asm);
        define_bss_init_done(&asm);
    }
    define_thread_pointer_block(&asm);
    if asm.rt_config.multihart_reset_handling_required() {
        build_multi_hart_start(&asm);
    } else {
        build_boot_hart_start(&asm);
        if asm.rt_config.is_multi_hart() {
            build_secondary_hart_start(&asm);
        }
    }

    asm.release_id_regs();

    if asm.rt_config.needs_stack_overflow_detection() {
        protect_stack_section(&asm);
    }

    // Park harts
    park_hart(&asm);

    restore_trap_frame(&asm);
    handle_trap(&asm);
    goto_rust_entrypoint(&asm);

    write_asm_helpers(&asm);
    create_trap_frame(&asm);
    asm.generate(&fw);
    fw.write()
}

fn write_asm_rs_file(
    dirpath: &Path,
    boot_s_filename: &str,
    root_fw: &FileWriter,
) -> std::io::Result<()> {
    let asm_rs_filename = "asm.rs";
    let filepath = dirpath.join(asm_rs_filename);
    let fw = FileWriter::new(filepath.clone(), BlockDelimiter::Parens);
    fw.add_line(&format!("// {}", auto_generate_banner()));
    fw.add_line(&format!(
        "core::arch::global_asm!(include_str!({boot_s_filename:?}));"
    ));
    add_module(root_fw, &filepath);
    fw.write()
}

fn getter_func_name(member_name: &str) -> String {
    format!("get_{member_name:#}")
}

fn setter_func_name(member_name: &str) -> String {
    format!("set_{member_name:#}")
}

fn define_getter(rust: &RustBuilder, member_name: &str) {
    rust.new_method_with_ret(getter_func_name(member_name), "usize".to_string());
    rust.get_self_member(member_name.to_string());
    rust.end_method();
}

fn define_setter(rust: &RustBuilder, member_name: &str) {
    rust.new_method_self_mut_with_arg(setter_func_name(member_name), "val: usize".to_string());
    rust.set_self_member(member_name.to_string(), "val".to_string());
    rust.end_method();
}

fn define_struct(rust: &RustBuilder, name: String, members: Vec<String>, define_reset_func: bool) {
    rust.new_struct(name.to_string());
    for member in &members {
        rust.new_struct_field(member.to_string(), "usize".to_string());
    }
    rust.end_struct();

    rust.new_impl(name);
    for member in &members {
        define_getter(rust, member);
        define_setter(rust, member);
    }

    if define_reset_func {
        // Provide a helper for doing a reset of the entire struct
        rust.new_method_self_mut("reset".to_string());

        for member in &members {
            rust.call_without_ret(
                format!("self.{}", setter_func_name(member)),
                vec!["0".to_string()],
            );
        }

        rust.end_method();
    }

    rust.end_impl();
}

fn define_trapframe_helper(rust: &RustBuilder, rt_config: &RtConfig) {
    rust.new_func_with_ret(
        "trapframe".to_string(),
        format!("&'static mut {:#}", rt_config.trap_frame_rust_struct_name()),
    );
    rust.new_unsafe_block();
    rust.implicit_ret(format!(
        "&mut *(super::{:#}() as *mut {:#})",
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::TrapFrameAddr),
        rt_config.trap_frame_rust_struct_name()
    ));
    rust.end_unsafe_block();
    rust.end_func();
}

fn write_trapframe_rs_file(
    dirpath: &Path,
    rt_config: &RtConfig,
    root_fw: &FileWriter,
) -> std::io::Result<()> {
    let trapframe_rs_filename = "trapframe.rs";
    let filepath = dirpath.join(trapframe_rs_filename);
    let fw = FileWriter::new(filepath.clone(), BlockDelimiter::Parens);

    let rust = RustBuilder::new();

    define_struct(
        &rust,
        rt_config.trap_frame_rust_struct_name(),
        rt_config.trap_frame_members(),
        true,
    );

    define_trapframe_helper(&rust, rt_config);
    RtFlagBit::generate(&rust);

    rust.generate(&fw);

    add_module(root_fw, &filepath);
    fw.write()
}

fn rust_tp_block_slice(rust: &RustBuilder, rt_config: &RtConfig) {
    let asm_fn = GEN_FUNC_MAP.asm_fn(GeneratedFunc::TpBlockBase);

    rust.new_c_extern();
    rust.func_prototype(asm_fn.clone(), Vec::new(), Some("usize".to_string()));
    rust.end_extern();

    rust.new_func_with_ret(
        GEN_FUNC_MAP.rust_fn(GeneratedFunc::TpBlockSlice),
        format!("&'static [{:#}]", rt_config.tp_block.rust_struct_name()),
    );
    rust.new_unsafe_block();
    rust.implicit_ret(format!(
        "core::slice::from_raw_parts({:#}() as *const _,{:#})",
        asm_fn,
        rt_config.max_hart_count(),
    ));
    rust.end_unsafe_block();
    rust.end_func();
}

fn rust_hartid_map(rust: &RustBuilder, fn_name: &str, src: TpBlockMember, dst: TpBlockMember) {
    let id_arg = "id";

    rust.new_func_with_arg_and_ret(
        fn_name.to_string(),
        format!("{id_arg:#}: usize"),
        "Option<usize>".to_string(),
    );

    let var_tp_element = "tp";

    rust.for_iter(
        var_tp_element,
        &format!("{:#}()", GEN_FUNC_MAP.rust_fn(GeneratedFunc::TpBlockSlice)),
    );
    rust.if_eq(&format!("{var_tp_element:#}.get_{src:#}()"), id_arg);

    rust.explicit_ret(format!("Some({var_tp_element:#}.get_{dst:#}())"));

    rust.end_if();
    rust.end_for();

    rust.implicit_ret("None".to_string());
    rust.end_func();
}

fn rust_boot_to_hart_id(rust: &RustBuilder) {
    rust_hartid_map(
        rust,
        "boot_to_hart_id",
        TpBlockMember::BootId,
        TpBlockMember::HartId,
    );
}

fn rust_hart_to_boot_id(rust: &RustBuilder) {
    rust_hartid_map(
        rust,
        "hart_to_boot_id",
        TpBlockMember::HartId,
        TpBlockMember::BootId,
    );
}

fn write_tpblock_rust_helpers(rust: &RustBuilder, rt_config: &RtConfig) {
    rust_my_ids(rust);
    rust_my_trap_frame_addr(rust);
    rust_my_tp_block_addr(rust);
    rust_get_rest_tf_label(rust);
    rust_tp_block_mut(rust, rt_config);
    rust_tp_block_slice(rust, rt_config);
    rust_boot_to_hart_id(rust);
    rust_hart_to_boot_id(rust);
    rust_switch_to(rust, "ctx".to_string());
}

fn write_tpblock_rs_file(
    dirpath: &Path,
    rt_config: &RtConfig,
    root_fw: &FileWriter,
) -> std::io::Result<()> {
    let tpblock_rs_filename = "tpblock.rs";
    let filepath = dirpath.join(tpblock_rs_filename);
    let fw = FileWriter::new(filepath.clone(), BlockDelimiter::Parens);

    let rust = RustBuilder::new();

    define_struct(
        &rust,
        rt_config.tp_block.rust_struct_name(),
        rt_config.tp_block.members(),
        false,
    );

    write_tpblock_rust_helpers(&rust, rt_config);
    rust.generate(&fw);

    add_module(root_fw, &filepath);
    fw.write()
}

fn export_max_boot_ids(rt_config: &RtConfig, root_fw: &FileWriter) {
    root_fw.add_line("#[allow(dead_code)]");
    root_fw.add_line(&format!(
        "pub const MAX_BOOT_IDS: usize = {};",
        rt_config.target_config.max_hart_count()
    ));
}

pub fn write_rt_files(
    dirpath_name: &str,
    rt_config: &RtConfig,
    crate_type: CrateType,
) -> std::io::Result<()> {
    let dirpath = PathBuf::from(dirpath_name);
    let boot_s_filename = "boot.S";
    let root_fw = create_root_rs_filewriter(&dirpath, crate_type);

    write_boot_s_file(&dirpath, rt_config, boot_s_filename)?;
    write_asm_rs_file(&dirpath, boot_s_filename, &root_fw)?;
    write_tpblock_rs_file(&dirpath, rt_config, &root_fw)?;
    write_trapframe_rs_file(&dirpath, rt_config, &root_fw)?;
    export_max_boot_ids(rt_config, &root_fw);
    root_fw.write()
}
