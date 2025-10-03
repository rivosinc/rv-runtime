use core::fmt::Write;
use lazy_static::lazy_static;
use spin::Mutex;

use crate::{my_boot_id, my_hart_id};

struct QemuUart {
    base: usize,
}

impl QemuUart {
    const fn new() -> Self {
        Self { base: 0x1000_0000 }
    }

    fn write(&self, b: u8) {
        let ptr = self.base as *mut u8;
        unsafe {
            ptr.write_volatile(b);
        }
    }
}

lazy_static! {
    static ref UART: Mutex<QemuUart> = Mutex::new(QemuUart::new());
}

#[macro_export]
macro_rules! println {
    () => (print!("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::io::_print(format_args!($($arg)*)));
}

impl core::fmt::Write for QemuUart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            self.write(b);
        }
        Ok(())
    }
}

pub fn _print(args: core::fmt::Arguments) {
    // Explicitly ignore errors here.
    let _ = UART.lock().write_fmt(args);
}

pub struct UartLogger;

impl log::Log for UartLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        println!("H{}:B{} - {}", my_hart_id(), my_boot_id(), record.args());
    }

    fn flush(&self) {}
}
