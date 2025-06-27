use rv_runtime_generator::*;
use std::collections::HashMap;

fn main() {
    let alignment = 4096;
    let max_hart_count = 4;
    let per_hart_stack_size = 8192;
    let heap_size = 4096;
    let target_config = TargetConfig {
        hart_config: HartConfig::new(RvMode::MMode, RvXlen::Rv64, max_hart_count, true),
        mem_config: MemConfig::new(per_hart_stack_size, heap_size),
        custom_reset_config: true,
    };

    let runtime_config = RuntimeConfig {
        rt_dirpath_name: "src/generated/rt",
        linker_dirpath_name: "src/generated/linker",
        linker_config: LinkerConfig::new(
            vec![
                MemoryRegion::new(
                    "region_1",
                    0x8000_0000,
                    128 * KiB,
                    true,
                    MemoryAttribs::rx(),
                    Vec::new(),
                ),
                MemoryRegion::new(
                    "region_2",
                    0x8002_0000,
                    64 * KiB,
                    true,
                    MemoryAttribs::rw(),
                    vec![
                        SubRegion::new("subregion_1", 56 * KiB, false),
                        SubRegion::new("subregion_2", 8 * KiB, true),
                    ],
                ),
            ],
            vec![
                Section::new(SectionType::Text, alignment, "region_1"),
                Section::new(SectionType::Rodata, alignment, "region_1"),
                Section::new(SectionType::Data, alignment, "subregion_1"),
                Section::new(SectionType::Bss, alignment, "subregion_1"),
                Section::new(SectionType::Heap, alignment, "subregion_1"),
                Section::new(
                    SectionType::Custom("custom_section".to_string(), 4096),
                    alignment,
                    "subregion_1",
                ),
            ],
            StackLocation::InBss(StackAlignment::Natural),
            target_config.clone(),
        ),
        rt_config: RtConfig::new(
            HashMap::from([
                (EntrypointType::BootHart, "main".to_string()),
                (EntrypointType::NonBootHart, "secondary_main".to_string()),
                (EntrypointType::Trap, "trap_enter".to_string()),
                (EntrypointType::CustomReset, "my_custom_reset".to_string()),
                (
                    EntrypointType::StackOverflow,
                    "handle_stack_overflow".to_string(),
                ),
            ]),
            TrapFrame::get_default(),
            TpBlock::get_default(),
            ThreadContext::get_default(),
            target_config,
            false,
            false,
            true,
            true,
            false,
        ),
    };

    std::fs::create_dir_all(runtime_config.rt_dirpath_name)
        .expect("Failed to create generated directory");
    std::fs::create_dir_all(runtime_config.linker_dirpath_name)
        .expect("Failed to create generated directory");
    write_linker_files(
        runtime_config.linker_dirpath_name,
        &runtime_config.linker_config,
        CrateType::Module,
    )
    .expect("Failed to write linker files");
    write_rt_files(
        runtime_config.rt_dirpath_name,
        &runtime_config.rt_config,
        CrateType::Module,
    )
    .expect("Failed to write rt files");

    println!("cargo:rerun-if-changed={}", runtime_config.rt_dirpath_name);
    println!(
        "cargo:rerun-if-changed={}",
        runtime_config.linker_dirpath_name
    );
}
