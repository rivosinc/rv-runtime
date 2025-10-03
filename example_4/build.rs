use rv_runtime_generator::*;
use std::collections::HashMap;

fn main() {
    /* Assuming an alignment requirement of 4KiB for each section */
    let alignment = 4096;
    let hart_config = HartConfig::new(RvMode::MMode, RvXlen::Rv64)
        .set_max_hart_count(4)
        .set_all_harts_start_at_reset_vector();
    let mem_config = MemConfig::new()
        .set_per_hart_stack_size(8192)
        .set_heap_size(4096);
    let target_config = TargetConfig::new(hart_config, mem_config).set_custom_reset_config();

    let memory_regions = vec![
        MemoryRegion::new("region_1")
            .set_base(0x8000_0000)
            .set_length(128 * KiB)
            .set_napot()
            .set_memory_attribs(MemoryAttribs::rx()),
        MemoryRegion::new("region_2")
            .set_base(0x8002_0000)
            .set_length(64 * KiB)
            .set_napot()
            .set_memory_attribs(MemoryAttribs::rw())
            .set_sub_regions(vec![
                SubRegion::new("subregion_1", 56 * KiB, false),
                SubRegion::new("subregion_2", 8 * KiB, true),
            ]),
    ];

    let sections = vec![
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
    ];

    let entrypoints = HashMap::from([
        (EntrypointType::BootHart, "main".to_string()),
        (EntrypointType::Trap, "trap_enter".to_string()),
        (EntrypointType::CustomReset, "my_custom_reset".to_string()),
        (
            EntrypointType::StackOverflow,
            "handle_stack_overflow".to_string(),
        ),
    ]);

    let linker_config = LinkerConfig::new(
        memory_regions,
        sections,
        StackLocation::InBss(StackAlignment::Natural),
        target_config.clone(),
    );
    let rt_config = RtConfig::new(target_config)
        .set_entrypoints(entrypoints);
    let runtime_config = RuntimeConfig {
        rt_dirpath_name: "src/generated/rt",
        linker_dirpath_name: "src/generated/linker",
        linker_config,
        rt_config,
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
