#include <stdint.h>

#define UART_BASE 0x10000000
#define UART_THR  (UART_BASE + 0x00)

static void volatile_write8(uintptr_t addr, uint8_t data) {
  *(volatile uint8_t *)addr = data;
}

static void volatile_write32(uintptr_t addr, uint32_t data) {
  *(volatile uint32_t *)addr = data;
}

static inline void uart_putc(char c) {
  volatile_write8(UART_THR, c);
}

static void uart_puts(const char *s) {
  while (*s) {
    uart_putc(*s++);
  }
}

static void poweroff(void) {
  uart_puts("Powering off!\n");
  volatile_write32(0x100000, 0x5555);
}

void main(void) {
  uart_puts("Hello, RISC-V World!\n");
  poweroff();
}

void trap_enter(void) {
  uart_puts("RISC-V trap!\n");
  // Hang forever
  while (1) {
    __asm__ volatile ("wfi");
  }
}
