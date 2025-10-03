PREFIX = riscv64-unknown-elf-

# Tools
CC = $(PREFIX)gcc
AS = $(PREFIX)as
LD = $(PREFIX)ld
OBJCOPY = $(PREFIX)objcopy

# Flags
CFLAGS = -march=rv64g -mabi=lp64 -nostdlib -ffreestanding -O2 -mcmodel=medany
LDFLAGS = -Tsrc/generated/linker/program.ld -nostdlib

# Output
TARGET = hello.elf
BINARY = hello.bin

all: $(TARGET) $(BINARY)

$(TARGET): boot.o main.o
	$(LD) $(LDFLAGS) -o $@ $^

boot.o: src/generated/rt/boot.S
	$(CC) $(CFLAGS) -c -o $@ $<

main.o: src/main.c
	$(CC) $(CFLAGS) -c -o $@ $<

$(BINARY): $(TARGET)
	$(OBJCOPY) -O binary $< $@

clean:
	rm -f *.o $(TARGET) $(BINARY)

run: $(TARGET)
	qemu-system-riscv64 -machine virt -bios none -kernel $(TARGET) -nographic

.PHONY: all clean run
