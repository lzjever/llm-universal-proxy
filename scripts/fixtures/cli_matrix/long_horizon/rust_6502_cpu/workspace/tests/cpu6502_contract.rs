use cpu6502::{Cpu6502, MemoryBus, StatusFlags};

fn cpu_with_program(program: &[u8]) -> (Cpu6502, MemoryBus) {
    let mut cpu = Cpu6502::new();
    let mut bus = MemoryBus::new();
    bus.load(0x8000, program);
    bus.write(0xfffc, 0x00);
    bus.write(0xfffd, 0x80);
    cpu.reset(&mut bus);
    (cpu, bus)
}

#[test]
fn reset_and_lda_immediate_update_registers_and_flags() {
    let (mut cpu, mut bus) = cpu_with_program(&[0xa9, 0x42, 0xa9, 0x00, 0xa9, 0x80, 0x00]);

    assert_eq!(cpu.pc, 0x8000);
    assert_eq!(cpu.sp, 0xfd);
    assert!(cpu.status.contains(StatusFlags::UNUSED));
    assert!(cpu.status.contains(StatusFlags::INTERRUPT_DISABLE));

    assert_eq!(cpu.step(&mut bus), 2);
    assert_eq!(cpu.a, 0x42);
    assert_eq!(cpu.pc, 0x8002);
    assert!(!cpu.status.contains(StatusFlags::ZERO));
    assert!(!cpu.status.contains(StatusFlags::NEGATIVE));

    assert_eq!(cpu.step(&mut bus), 2);
    assert_eq!(cpu.a, 0x00);
    assert!(cpu.status.contains(StatusFlags::ZERO));
    assert!(!cpu.status.contains(StatusFlags::NEGATIVE));

    assert_eq!(cpu.step(&mut bus), 2);
    assert_eq!(cpu.a, 0x80);
    assert!(!cpu.status.contains(StatusFlags::ZERO));
    assert!(cpu.status.contains(StatusFlags::NEGATIVE));
}

#[test]
fn arithmetic_sets_carry_overflow_negative_and_zero_flags() {
    let (mut cpu, mut bus) = cpu_with_program(&[
        0xa9, 0x50, // LDA #$50
        0x18, // CLC
        0x69, 0x10, // ADC #$10 => $60
        0x69, 0x50, // ADC #$50 => $b0, signed overflow
        0x38, // SEC
        0xe9, 0x20, // SBC #$20 => $90, no borrow
        0x49, 0x90, // EOR #$90 => $00
        0x00, // BRK
    ]);

    cpu.step(&mut bus);
    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!(cpu.a, 0x60);
    assert!(!cpu.status.contains(StatusFlags::CARRY));
    assert!(!cpu.status.contains(StatusFlags::OVERFLOW));

    cpu.step(&mut bus);
    assert_eq!(cpu.a, 0xb0);
    assert!(!cpu.status.contains(StatusFlags::CARRY));
    assert!(cpu.status.contains(StatusFlags::OVERFLOW));
    assert!(cpu.status.contains(StatusFlags::NEGATIVE));

    cpu.step(&mut bus);
    cpu.step(&mut bus);
    assert_eq!(cpu.a, 0x90);
    assert!(cpu.status.contains(StatusFlags::CARRY));
    assert!(!cpu.status.contains(StatusFlags::OVERFLOW));
    assert!(cpu.status.contains(StatusFlags::NEGATIVE));

    cpu.step(&mut bus);
    assert_eq!(cpu.a, 0x00);
    assert!(cpu.status.contains(StatusFlags::ZERO));
}

#[test]
fn zero_page_branching_stack_and_run_until_brk_work_together() {
    let (mut cpu, mut bus) = cpu_with_program(&[
        0xa9, 0x03, // LDA #3
        0x85, 0x10, // STA $10
        0xa2, 0x00, // LDX #0
        0xe8, // loop: INX
        0xe0, 0x03, // CPX #3
        0xd0, 0xfb, // BNE loop
        0xa5, 0x10, // LDA $10
        0x48, // PHA
        0xa9, 0x00, // LDA #0
        0x68, // PLA
        0x00, // BRK
    ]);

    let executed = cpu
        .run_until_brk(&mut bus, 64)
        .expect("program should reach BRK before the limit");

    assert!(executed >= 12);
    assert_eq!(cpu.x, 0x03);
    assert_eq!(cpu.a, 0x03);
    assert_eq!(cpu.sp, 0xfd);
    assert_eq!(bus.read(0x0010), 0x03);
    assert!(!cpu.status.contains(StatusFlags::ZERO));
    assert!(!cpu.status.contains(StatusFlags::NEGATIVE));
}

#[test]
fn subroutines_absolute_store_and_register_transfers_are_supported() {
    let (mut cpu, mut bus) = cpu_with_program(&[
        0x20, 0x09, 0x80, // JSR $8009
        0x8d, 0x00, 0x02, // STA $0200
        0xaa, // TAX
        0xe8, // INX
        0x00, // BRK
        0xa9, 0x7f, // subroutine: LDA #$7f
        0x60, // RTS
    ]);

    cpu.run_until_brk(&mut bus, 64)
        .expect("subroutine program should reach BRK");

    assert_eq!(bus.read(0x0200), 0x7f);
    assert_eq!(cpu.a, 0x7f);
    assert_eq!(cpu.x, 0x80);
    assert_eq!(cpu.sp, 0xfd);
}
