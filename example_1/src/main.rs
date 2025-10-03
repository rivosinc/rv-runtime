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

#[no_mangle]
pub extern "C" fn main() {
    logger_init();

    let trap_frame = trapframe();
    log::info!("Hello World from bare-metal start(boot hart)!",);

    log::info!("rt_flags in trapframe: {:#x?}", trap_frame.get_rt_flags());
    sbicall(0, 0);
    log::info!("back from sbi call");

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

    let mepc = trap_frame.get_mepc();
    trap_frame.set_mepc(mepc + 4);
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
