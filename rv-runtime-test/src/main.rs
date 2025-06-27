// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]

#[allow(unused_imports)]
#[rustfmt::skip]
mod generated;

use core::arch::asm;

use generated::*;
use io::UartLogger;

mod io;

#[no_mangle]
extern "C" fn eh_personality() {}

fn poweroff() -> ! {
    const QEMU_RESET_REG: usize = 0x0010_0000;
    const QEMU_POWEROFF_VAL: u32 = 0x0000_5555;
    let reset_addr = QEMU_RESET_REG as *mut u32;
    unsafe {
        core::ptr::write_volatile(reset_addr, QEMU_POWEROFF_VAL);
    }

    // Sometimes QEMU will execute a few more instructions after
    // writing to the magic poweroff register, so hang out here.
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    log::error!("panicked, powering off!");
    poweroff()
}

core::arch::global_asm!(include_str!("custom_reset.S"));

static UART_LOGGER: UartLogger = UartLogger;

fn logger_init() {
    // Intentionaly ignoring return.
    let _ = log::set_logger(&UART_LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

fn sbicall(eid: usize, fid: usize) -> (usize, usize) {
    let value: usize;
    let error: usize;

    unsafe {
        asm!("ecall",
             in("a6") fid, in("a7") eid,
             out("a0") error, out("a1") value
        );
    }
    (error, value)
}

const ONE_POINT_ZERO_AS_INT: u64 = 0x3f800000;
const TWO_POINT_ZERO_AS_INT: u64 = 0x40000000;

#[no_mangle]
pub extern "C" fn main() {
    logger_init();

    let trap_frame = trapframe();
    log::info!("Hello World from bare-metal start(boot hart)!",);

    // Write 1.0 into f0 and f31
    let one_point_zero = ONE_POINT_ZERO_AS_INT;
    unsafe {
        core::arch::asm!(
            "fmv.d.x f0, {0}",
            "fmv.d.x f31, {0}",
            in(reg) one_point_zero,
            out("f0") _,
            out("f31") _,
        );
    }

    // Read back f0/f31 to ensure they are correct
    let f0: u64;
    let f31: u64;
    unsafe {
        core::arch::asm!(
            "fmv.x.d {0}, f0",
            "fmv.x.d {1}, f31",
            out(reg) f0,
            out(reg) f31,
            out("f0") _,
            out("f31") _,
        );
    }
    assert_eq!(f0, ONE_POINT_ZERO_AS_INT);
    assert_eq!(f31, ONE_POINT_ZERO_AS_INT);

    log::info!("rt_flags in trapframe: {:#x?}", trap_frame.get_rt_flags());
    sbicall(0, 0);
    log::info!("back from sbi call");

    // Ensure f0/31 is still the same value
    let f0: u64;
    let f31: u64;
    unsafe {
        core::arch::asm!(
            "fmv.x.d {0}, f0",
            "fmv.x.d {1}, f31",
            out(reg) f0,
            out(reg) f31
        );
    }
    log::info!("f0  == {:#x?}", f0);
    log::info!("f31 == {:#x?}", f31);
    assert_eq!(f0, ONE_POINT_ZERO_AS_INT);
    assert_eq!(f31, ONE_POINT_ZERO_AS_INT);

    log::info!("powering off");

    poweroff();
}

#[no_mangle]
pub extern "C" fn test_main_mret() -> ! {
    logger_init();

    log::info!("Hello World from bare-metal mret!");
    log::info!("rt_flags in trapframe: {:#x?}", trapframe().get_rt_flags());
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

#[no_mangle]
pub extern "C" fn secondary_main() {
    logger_init();

    log::info!("Hello World from bare-metal start(secondary)!",);
}

#[no_mangle]
pub extern "C" fn trap_enter() {
    let trap_frame = trapframe();

    log::info!("Hello World from trap!");
    log::info!("rt_flags in trapframe: {:#x?}", trap_frame.get_rt_flags());
    log::info!("f0 in trapframe : {:#x?}", trap_frame.get_f0());
    log::info!("f31 in trapframe: {:#x?}", trap_frame.get_f31());
    assert_eq!(trap_frame.get_f0(), ONE_POINT_ZERO_AS_INT as usize);
    assert_eq!(trap_frame.get_f31(), ONE_POINT_ZERO_AS_INT as usize);

    let mepc = trap_frame.get_mepc();
    trap_frame.set_mepc(mepc + 4);

    // Write a different value into f0/31. This allows us to verify that the restore path
    // is correctly restoring at least f0/31.
    let two_point_zero = TWO_POINT_ZERO_AS_INT;
    unsafe {
        core::arch::asm!(
            "fmv.d.x f0, {0}",
            "fmv.d.x f31, {0}",
            in(reg) two_point_zero,
            out("f0") _,
            out("f31") _,
        );
    }

    // Read back f0/31 to ensure it's correct
    let f0: u64;
    let f31: u64;
    unsafe {
        core::arch::asm!(
            "fmv.x.d {0}, f0",
            "fmv.x.d {1}, f31",
            out(reg) f0,
            out(reg) f31,
            out("f0") _,
            out("f31") _,
        );
    }
    log::info!("overwritten f0 in trap: {:#x?}", f0);
    log::info!("overwritten f31 in trap: {:#x?}", f31);
    assert_eq!(f0, TWO_POINT_ZERO_AS_INT);
    assert_eq!(f31, TWO_POINT_ZERO_AS_INT);
}

/// Entry point for handling stack overflow
#[no_mangle]
pub extern "C" fn handle_stack_overflow(expected_val: usize, stack_bottom_val: usize) {
    log::error!(
        "stack overflow detected: expected val: {:#x?}, stack bottom val: {:#x?}",
        expected_val,
        stack_bottom_val
    );
    panic!();
}
