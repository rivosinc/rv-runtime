// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::crate_type::*;
use crate::file_writer::*;
use crate::func::*;
use crate::rust::*;
use crate::target_config::*;

#[derive(Debug, Clone, Copy, Default)]
pub struct MemoryAttribs {
    read: bool,
    write: bool,
    execute: bool,
    allocated: bool,
    initialized: bool,
}

impl MemoryAttribs {
    pub fn r() -> Self {
        MemoryAttribs {
            read: true,
            ..Default::default()
        }
    }

    pub fn w() -> Self {
        MemoryAttribs {
            write: true,
            ..Default::default()
        }
    }

    pub fn rw() -> Self {
        MemoryAttribs {
            read: true,
            write: true,
            ..Default::default()
        }
    }

    pub fn x() -> Self {
        MemoryAttribs {
            execute: true,
            ..Default::default()
        }
    }

    pub fn rx() -> Self {
        MemoryAttribs {
            read: true,
            execute: true,
            ..Default::default()
        }
    }

    pub fn rwx() -> Self {
        MemoryAttribs {
            read: true,
            write: true,
            execute: true,
            ..Default::default()
        }
    }
}

impl std::fmt::Display for MemoryAttribs {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let mut print_str = String::new();
        if self.read {
            print_str.push('r');
        }
        if self.write {
            print_str.push('w');
        }
        if self.execute {
            print_str.push('x');
        }
        if self.allocated {
            print_str.push('a');
        }
        if self.initialized {
            print_str.push('i');
        }
        write!(f, "{print_str}")
    }
}

#[allow(non_upper_case_globals)]
pub const KiB: usize = 1024;
#[allow(non_upper_case_globals)]
pub const MiB: usize = KiB * 1024;

fn is_aligned(val: usize, alignment: usize) -> bool {
    (val % alignment) == 0
}

fn is_power_of_2(val: usize) -> bool {
    (val & (val - 1)) == 0
}

fn check_napot(name: &str, base: usize, length: usize) {
    assert!(
        is_power_of_2(length),
        "Memory {name:#} has a length {length:#x} which is not a power-of-2"
    );
    assert!(
        is_aligned(base, length),
        "Memory {name:#} has a base {base:#x} which is not aligned to length {length:#x}"
    );
}

#[derive(Debug)]
pub struct SubRegion {
    name: String,
    length: usize,
    napot: bool,
}

impl SubRegion {
    pub fn new(name: &str, length: usize, napot: bool) -> Self {
        Self {
            name: name.to_string(),
            length,
            napot,
        }
    }
}

#[derive(Debug)]
pub struct MemoryRegion {
    name: String,
    base: usize,
    length: usize,
    napot: bool,
    attribs: MemoryAttribs,
    sub_regions: Vec<SubRegion>,
}

impl MemoryRegion {
    pub fn new(
        name: &str,
        base: usize,
        length: usize,
        napot: bool,
        attribs: MemoryAttribs,
        sub_regions: Vec<SubRegion>,
    ) -> Self {
        Self {
            name: name.to_string(),
            base,
            length,
            napot,
            attribs,
            sub_regions,
        }
    }

    fn end(&self) -> usize {
        self.base + self.length
    }
}

#[derive(Debug)]
pub struct Memory<'a> {
    name: String,
    base: usize,
    length: usize,
    attribs: MemoryAttribs,
    sections: RefCell<Vec<&'a Section>>,
}

impl<'a> Memory<'a> {
    fn new(name: &str, base: usize, length: usize, attribs: MemoryAttribs) -> Self {
        Self {
            name: name.to_string(),
            base,
            length,
            attribs,
            sections: RefCell::new(Vec::new()),
        }
    }

    fn first_section_start_symbol(&self) -> String {
        self.sections
            .borrow()
            .first()
            .unwrap()
            .ty
            .section_entry_start_symbol()
    }

    fn last_section_end_symbol(&self) -> String {
        self.sections
            .borrow()
            .last()
            .unwrap()
            .ty
            .section_entry_end_symbol()
    }

    fn is_empty(&self) -> bool {
        self.sections.borrow().is_empty()
    }

    fn from_memory_region(region: &MemoryRegion) -> Vec<Self> {
        if region.napot {
            check_napot(&region.name, region.base, region.length);
        }

        let mut memories = Vec::new();

        memories.push(Self::new(
            &region.name,
            region.base,
            region.length,
            region.attribs,
        ));

        let mut base = region.base;

        for sub_region in &region.sub_regions {
            if sub_region.napot {
                assert!(
                    region.napot,
                    "NAPOT sub-region {:?} inside a non-NAPOT region {:?}",
                    sub_region.name, region.name
                );
                check_napot(&sub_region.name, base, sub_region.length);
            }

            assert!(sub_region.length + base <= region.end(), "Sub-region base {:#x} length {:#x} overflows encompassing region base {:#x} length {:#x}", base, sub_region.length, region.base, region.length);
            memories.push(Self::new(
                &sub_region.name,
                base,
                sub_region.length,
                region.attribs,
            ));

            base += sub_region.length;
        }

        memories
    }

    pub fn start_symbol(&self) -> String {
        format!("_s{:#}", self.name)
    }

    pub fn end_symbol(&self) -> String {
        format!("_e{:#}", self.name)
    }

    fn add_section(&self, section: &'a Section) {
        self.sections.borrow_mut().push(section);
    }

    fn base(&self) -> usize {
        self.base
    }

    fn end(&self) -> usize {
        self.base + self.length
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl<'a> std::fmt::Display for Memory<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "{:#} ({:#}) : ORIGIN = {:#x}, LENGTH = {:#x}",
            self.name, self.attribs, self.base, self.length
        )
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SectionType {
    Text,
    Data,
    Rodata,
    Bss,
    Heap,
    Stack,
    Custom(String, usize),
}

pub fn program_start_symbol() -> String {
    "_sprogram".to_string()
}

pub fn program_end_symbol() -> String {
    "_eprogram".to_string()
}

pub fn stack_top_symbol() -> String {
    "_stack_top".to_string()
}

pub fn global_pointer_symbol() -> String {
    "_global_pointer".to_string()
}

pub fn reset_section() -> String {
    ".text.entry".to_string()
}

pub fn custom_reset_section() -> String {
    ".text.custom_reset_entry".to_string()
}

pub fn text_default_section() -> String {
    let sections = SectionType::Text.default_sections();
    sections[0].to_string()
}

pub fn data_default_section() -> String {
    let sections = SectionType::Data.default_sections();
    sections[0].to_string()
}

impl SectionType {
    pub fn name(&self) -> &str {
        match self {
            Self::Text => "text",
            Self::Data => "data",
            Self::Rodata => "rodata",
            Self::Bss => "bss",
            Self::Heap => "heap",
            Self::Stack => "stack",
            Self::Custom(name, _) => name,
        }
    }

    fn default_sections(&self) -> Vec<&str> {
        match self {
            Self::Text => vec![".text"],
            Self::Data => vec![".data", ".sdata"],
            Self::Rodata => vec![".rodata", ".srodata"],
            Self::Bss => vec![".bss", ".sbss"],
            Self::Heap | Self::Stack | Self::Custom(_, _) => Vec::new(),
        }
    }

    pub fn section_entry_name(&self) -> String {
        format!(".{:#}", self.name())
    }

    pub fn section_entry_start_symbol(&self) -> String {
        format!("_s{:#}", self.name())
    }

    pub fn section_entry_end_symbol(&self) -> String {
        format!("_e{:#}", self.name())
    }
}

// Subsections can be added to Sections to be included in the linker script. They only have
// alignment and an input section name.
#[derive(Debug, Clone)]
pub struct SubSection {
    input_section: String,
    alignment_in_bytes: usize,
    max_size: Option<usize>,
    mark_as_keep: bool,
}

impl SubSection {
    pub fn new(
        input_section_name: &str,
        alignment_in_bytes: usize,
        max_size: Option<usize>,
    ) -> Self {
        Self {
            input_section: input_section_name.to_string(),
            alignment_in_bytes,
            max_size,
            mark_as_keep: false,
        }
    }

    // Consume this object, and returns a new object with the
    // "mark_as_keep" flag set. This allows the struct to be used like
    // a builder pattern:
    //
    // payload_section
    //     .add_subsection(SubSection::new(".payload.data", alignment, None).keep());
    pub fn keep(mut self) -> Self {
        self.mark_as_keep = true;
        self
    }
}

// Deals with standard sections defined by the section type above. If custom sections are required for any purpose,
// best to add that as a separate structure for CustomSection.
#[derive(Debug, Clone)]
pub struct Section {
    ty: SectionType,
    start_alignment_in_bytes: usize,
    end_alignment_in_bytes: usize,
    target_memory: String,
    subsections: Vec<SubSection>,
    load_address: Option<String>, // Symbol indicating load address
}

impl Section {
    pub fn new(ty: SectionType, alignment_in_bytes: usize, target_memory: &str) -> Self {
        Self {
            ty,
            start_alignment_in_bytes: alignment_in_bytes,
            // By default we assume that start and end alignments are same. Later on, we can evaluate
            // if end alignment needs to be different based on any other requirements
            end_alignment_in_bytes: alignment_in_bytes,
            target_memory: target_memory.to_string(),
            subsections: Vec::new(),
            load_address: None,
        }
    }

    pub fn add_subsection(&mut self, subsection: SubSection) {
        self.subsections.push(subsection);
    }

    // Use the builder pattern to add a load address to this section
    pub fn with_load_address(mut self, load_address: &str) -> Self {
        self.load_address = Some(load_address.to_string());
        self
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackAlignment {
    // Align on 4KiB boundary
    #[default]
    Default,
    Natural,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackLocation {
    SeparateSection,
    InBss(StackAlignment),
}

impl Default for StackLocation {
    fn default() -> Self {
        Self::InBss(StackAlignment::Default)
    }
}

impl StackLocation {
    fn is_stack_in_bss(&self) -> bool {
        match self {
            StackLocation::InBss(_) => true,
            StackLocation::SeparateSection => false,
        }
    }

    fn is_stack_in_separate_section(&self) -> bool {
        !self.is_stack_in_bss()
    }
}

#[derive(Debug)]
pub struct Symbol {
    pub name: String,
    pub value: String,
}

impl Symbol {
    pub fn new(name: &str, value: &str) -> Self {
        Self {
            name: name.to_string(),
            value: value.to_string(),
        }
    }
}

#[derive(Debug)]
pub struct LinkerConfig<'a> {
    pub memories: Vec<Memory<'a>>,
    pub sections: Vec<Section>,
    pub stack_location: StackLocation,
    pub target_config: TargetConfig,
    pub symbols: Vec<Symbol>,
}

impl<'a> LinkerConfig<'a> {
    pub fn new(
        memory_regions: Vec<MemoryRegion>,
        mut sections: Vec<Section>,
        stack_location: StackLocation,
        target_config: TargetConfig,
    ) -> Self {
        let mut memories = Vec::new();
        let mut region_iter = memory_regions.iter().peekable();

        while let Some(region) = region_iter.next() {
            memories.append(&mut Memory::from_memory_region(region));

            // If this is not a NAPOT region, we don't need to update any section end alignment.
            if !region.napot {
                continue;
            }

            // If we are dealing with a NAPOT region which is non-trailing i.e. region which is not the last in
            // region list, then we need to ensure that the last section in such a region is aligned to the size
            // of the NAPOT region. This is to ensure that we fill out the hole between the end of this NAPOT region
            // and the start of the next region so that the on-storage layout is the same as the in-memory layout.
            if region_iter.peek().is_none() {
                continue;
            }

            // If the region is a composite region made up of sub-regions, then use the last sub-region to find the
            // section whose end alignment needs to be set.
            let region_name = if region.sub_regions.is_empty() {
                &region.name
            } else {
                &region.sub_regions.last().unwrap().name
            };

            let mut found_section = false;

            // Update the end_alignment_in_bytes in the last section mapped to this region. Walk in reverse order so
            // that it is the first section that we encounter.
            for section in sections.iter_mut().rev() {
                if section.target_memory.eq(region_name) {
                    section.end_alignment_in_bytes = region.length;
                    found_section = true;
                    break;
                }
            }

            assert!(
                found_section,
                "Non-trailing NAPOT region {region_name:?} has no sections mapped to it."
            );
        }

        // Ensure all the memories are sorted by their base address.
        memories.sort_by(|a, b| a.base.cmp(&b.base));

        if stack_location.is_stack_in_separate_section() {
            assert!(
                sections.iter().any(|s| s.ty == SectionType::Stack),
                "No stack region provided (stack outside BSS)!"
            );
        }

        Self {
            memories,
            sections,
            stack_location,
            target_config,
            symbols: vec![],
        }
    }

    pub fn section_types(&self) -> Vec<SectionType> {
        let mut sections = Vec::new();

        for section in &self.sections {
            sections.push(section.ty.clone());
        }

        if self.is_stack_in_bss() {
            sections.push(SectionType::Stack);
        }

        sections
    }

    fn hart_stack_size(&self) -> usize {
        self.target_config.per_hart_stack_size()
    }

    fn stack_region_size(&self) -> usize {
        self.hart_stack_size() * self.target_config.max_hart_count()
    }

    fn heap_size(&self) -> usize {
        self.target_config.heap_size()
    }

    fn stack_in_bss_alignment(&self) -> usize {
        match self.stack_location {
            StackLocation::InBss(StackAlignment::Default) => 4096, // 4KiB
            StackLocation::InBss(StackAlignment::Natural) => self.hart_stack_size(),
            StackLocation::SeparateSection => {
                panic!("Stack is not in BSS, the alignment of the section should be used instead")
            }
        }
    }

    fn is_stack_in_bss(&self) -> bool {
        self.stack_location.is_stack_in_bss()
    }

    pub fn add_symbol(&mut self, symbol: Symbol) {
        self.symbols.push(symbol);
    }
}

#[derive(Debug)]
enum Arch {
    Riscv,
}

impl std::fmt::Display for Arch {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let print_str = match self {
            Arch::Riscv => "riscv",
        };
        write!(f, "{print_str}")
    }
}

#[derive(Debug)]
enum LinkerSentence<'a> {
    OutputArch(Arch),         // (arch)
    Entry(String),            // (symbol)
    Memory(&'a [Memory<'a>]), // (slice of Memory structures)
    SectionsStart,
    SectionsEnd,
    OutputSectionStart(String, bool, usize, Option<String>), // (name, noload, alignment, load_address)
    OutputSectionEnd(String),                                // (target_memory)
    InputSections(String, bool),                             // (input sections string, keep)
    SetRelativeToLocationCounter(String, isize),             // (symbol, offset)
    SetToCurrent(String),                                    // (symbol)
    SetToValue(String, usize),                               // (symbol, value)
    SetToSymbol(String, String),                             // (symbol, symbol)
    AdvanceLocationCounter(usize),                           // (size)
    Align(usize),                                            // (alignment)
    Assert(String, String),                                  // (assert condition, error message)
    DiscardSectionStart,
    DiscardSectionEnd,
    Symbol(String, String), // (name, value expression)
    Comment(String),        // comment_string
}

impl<'a> LinkerSentence<'a> {
    fn generate(&self, fw: &FileWriter) {
        match self {
            Self::OutputArch(arch) => fw.add_line(&format!("OUTPUT_ARCH({arch:#})")),
            Self::Entry(symbol) => fw.add_line(&format!("ENTRY({symbol:#})")),
            Self::Memory(memories) => {
                fw.new_block("MEMORY");
                for memory in memories.iter() {
                    fw.add_line(&memory.to_string());
                }
                fw.end_block();
            }
            Self::SectionsStart => fw.new_block("SECTIONS"),
            Self::SectionsEnd => fw.end_block(),
            Self::OutputSectionStart(name, noload, alignment, load_address) => {
                let noload = if *noload { "(NOLOAD)" } else { "" };
                let load_addr = if let Some(symbol) = load_address {
                    format!("AT({symbol}) ")
                } else {
                    "".to_string()
                };
                fw.new_block(&format!(
                    "{name:#} {noload:#}: {load_addr}ALIGN({alignment:#})"
                ));
            }
            Self::OutputSectionEnd(target_memory) => {
                fw.end_block_with_suffix(&format!(">{target_memory:#}"))
            }
            Self::InputSections(sections, keep) => {
                if *keep {
                    fw.add_line(&format!("KEEP(*({sections:#}))"));
                } else {
                    fw.add_line(&format!("*({sections:#})"));
                }
            }
            Self::SetRelativeToLocationCounter(symbol, size) => {
                fw.add_line(&format!("{symbol:#} = . + {size:#x};"))
            }
            Self::SetToCurrent(symbol) => fw.add_line(&format!("{symbol:#} = .;")),
            Self::SetToValue(symbol, value) => fw.add_line(&format!("{symbol:#} = {value:#x};")),
            Self::SetToSymbol(symbola, symbolb) => {
                fw.add_line(&format!("{symbola:#} = {symbolb:#};"))
            }
            Self::AdvanceLocationCounter(size) => fw.add_line(&format!(". += {size:#x};")),
            Self::Align(alignment) => fw.add_line(&format!(". = ALIGN({alignment:#});")),
            Self::Assert(assert_cond, error_msg) => {
                fw.add_line(&format!("ASSERT({assert_cond:#}, {error_msg:?})"))
            }
            Self::DiscardSectionStart => fw.new_block("/DISCARD/ :"),
            Self::DiscardSectionEnd => fw.end_block(),
            Self::Symbol(name, value) => fw.add_line(&format!("{name} = {value};")),
            Self::Comment(comment) => fw.add_line(&format!("# {comment}")),
        }
    }
}

#[derive(Debug)]
struct LinkerBuilder<'a> {
    linker_config: &'a LinkerConfig<'a>,
    sentences: RefCell<Vec<LinkerSentence<'a>>>,
}

impl<'a> LinkerBuilder<'a> {
    fn new(linker_config: &'a LinkerConfig<'a>) -> Self {
        let lb = Self {
            linker_config,
            sentences: RefCell::new(Vec::new()),
        };
        lb.comment(&auto_generate_banner());
        lb
    }

    fn add_section_to_memory(&self, section: &'a Section) {
        for memory in &self.linker_config.memories {
            if memory.name.eq(&section.target_memory) {
                memory.add_section(section);
                return;
            }
        }

        panic!(
            "Target memory {:#} not found for {:#}",
            &section.target_memory,
            section.ty.name()
        );
    }

    fn generate(&self, fw: &FileWriter) {
        for sentence in self.sentences.borrow().iter() {
            sentence.generate(fw);
        }
    }

    fn add_sentence(&self, sentence: LinkerSentence<'a>) {
        self.sentences.borrow_mut().push(sentence)
    }

    fn output_arch(&self, arch: Arch) {
        self.add_sentence(LinkerSentence::OutputArch(arch));
    }

    fn entry(&self) {
        self.add_sentence(LinkerSentence::Entry(START_SYMBOL.to_string()));
    }

    fn memory(&self) {
        self.add_sentence(LinkerSentence::Memory(&self.linker_config.memories));
    }

    fn memory_symbols(&self) {
        for memory in &self.linker_config.memories {
            self.add_sentence(LinkerSentence::SetToValue(
                memory.start_symbol(),
                memory.base(),
            ));
            self.add_sentence(LinkerSentence::SetToValue(
                memory.end_symbol(),
                memory.end(),
            ));
        }
    }

    fn program_symbols(&self) {
        for memory in &self.linker_config.memories {
            if memory.sections.borrow().is_empty() {
                continue;
            }
            self.add_sentence(LinkerSentence::SetToSymbol(
                program_start_symbol(),
                memory.first_section_start_symbol(),
            ));
            break;
        }
        for memory in self.linker_config.memories.iter().rev() {
            if memory.sections.borrow().is_empty() {
                continue;
            }
            self.add_sentence(LinkerSentence::SetToSymbol(
                program_end_symbol(),
                memory.last_section_end_symbol(),
            ));
            break;
        }
    }

    fn output_section_start(
        &self,
        name: String,
        noload: bool,
        alignment: usize,
        load_address: Option<String>,
    ) {
        self.add_sentence(LinkerSentence::OutputSectionStart(
            name,
            noload,
            alignment,
            load_address,
        ));
    }

    fn output_section_end(&self, section_suffix: String) {
        self.add_sentence(LinkerSentence::OutputSectionEnd(section_suffix));
    }

    fn set_symbol_to_current(&self, symbol: String) {
        self.add_sentence(LinkerSentence::SetToCurrent(symbol));
    }

    fn set_symbol_offset_from_current(&self, symbol: String, offset: isize) {
        self.add_sentence(LinkerSentence::SetRelativeToLocationCounter(symbol, offset));
    }

    fn input_section(&self, section: &str, keep: bool) {
        self.add_sentence(LinkerSentence::InputSections(
            format!("{section:#} {section:#}.*"),
            keep,
        ));
    }

    fn align(&self, alignment: usize) {
        self.add_sentence(LinkerSentence::Align(alignment));
    }

    fn advance_location_counter(&self, size: usize) {
        self.add_sentence(LinkerSentence::AdvanceLocationCounter(size));
    }

    fn assert(&self, assert_cond: String, error_msg: String) {
        self.add_sentence(LinkerSentence::Assert(assert_cond, error_msg));
    }

    fn add_subsection_information(&self, section_info: &Section) {
        for ss in &section_info.subsections {
            // Subsection names may start with `.`
            // For subsection with name ".subsection", symbols are generated to mark start and
            // end of the subsection by replacing the `.` with `_s` and `_e`, respectively.
            let section_symbol_suffix = if ss.input_section.starts_with('.') {
                ss.input_section[1..].replace('.', "_")
            } else {
                ss.input_section.replace('.', "_")
            };

            // . = ALIGN(...);
            self.align(ss.alignment_in_bytes);

            // Start symbol for ".subsection" -> `_ssubsection`
            let start = format!("_s{section_symbol_suffix}");
            self.set_symbol_to_current(start.clone());

            self.input_section(&ss.input_section, ss.mark_as_keep);

            // . = ALIGN(...);
            self.align(ss.alignment_in_bytes);

            // End symbol for ".subsection" -> `_esubsection`
            let end = format!("_e{section_symbol_suffix}");
            self.set_symbol_to_current(end.clone());

            // ASSERT(_esubsection - _ssubsection <= max_size, "Section subsection exceeded max size");
            if let Some(max_size) = ss.max_size {
                self.assert(
                    format!("{end:#} - {start:#} <= {max_size:#}",),
                    format!("{:#} overflowed", ss.input_section),
                );
            }
        }
    }

    fn add_text_section(&self, section_info: &Section) {
        let ty = &section_info.ty;

        // .text : ALIGN(...) {
        self.output_section_start(
            ty.section_entry_name(),
            false,
            section_info.start_alignment_in_bytes,
            section_info.load_address.clone(),
        );

        // _stext =  .;
        self.set_symbol_to_current(ty.section_entry_start_symbol());

        // *(.text.entry .text.entry.*)
        self.input_section(&reset_section(), false);

        // *(.text.custom_reset_entry .text.custom_reset_entry.*)
        /*
         * It is the user component's responsibility to place the custom
         * reset entrypoint in .text.custom_reset_entry section so that it's
         * guaranteed to be kept close to the reset vector
         */
        if self.linker_config.target_config.needs_custom_reset() {
            self.input_section(&custom_reset_section(), false);
        }

        // *(.text .text.*)
        let default_sections = ty.default_sections();
        for input_section in default_sections {
            self.input_section(input_section, false);
        }

        // Handle all subsections */
        self.add_subsection_information(section_info);

        // . = ALIGN(...);
        self.align(section_info.end_alignment_in_bytes);

        // _etext = .;
        self.set_symbol_to_current(ty.section_entry_end_symbol());

        // } >{MEMORY}
        self.output_section_end(section_info.target_memory.to_string());
    }

    fn add_rodata_section(&self, section_info: &Section) {
        let ty = &section_info.ty;

        // .rodata : ALIGN(...) {
        self.output_section_start(
            ty.section_entry_name(),
            false,
            section_info.start_alignment_in_bytes,
            section_info.load_address.clone(),
        );

        // _srodata =  .;
        self.set_symbol_to_current(ty.section_entry_start_symbol());

        // *(.rodata .rodata.*)
        // *(.srodata .srodata.*)
        let default_sections = ty.default_sections();
        for input_section in default_sections {
            self.input_section(input_section, false);
        }

        // Handle all subsections */
        self.add_subsection_information(section_info);

        // . = ALIGN(...);
        self.align(section_info.end_alignment_in_bytes);

        // _erodata = .;
        self.set_symbol_to_current(ty.section_entry_end_symbol());

        // } >{MEMORY}
        self.output_section_end(section_info.target_memory.to_string());
    }

    fn add_data_section(&self, section_info: &Section) {
        let ty = &section_info.ty;

        // .data : ALIGN(...) {
        self.output_section_start(
            ty.section_entry_name(),
            false,
            section_info.start_alignment_in_bytes,
            section_info.load_address.clone(),
        );

        // _sdata =  .;
        self.set_symbol_to_current(ty.section_entry_start_symbol());

        // _global_pointer = . + 0x800;
        self.set_symbol_offset_from_current(global_pointer_symbol(), 0x800);

        // *(.data .data.*)
        // *(.sdata .sdata.*)
        let default_sections = ty.default_sections();
        for input_section in default_sections {
            self.input_section(input_section, false);
        }

        // Handle all subsections */
        self.add_subsection_information(section_info);

        // . = ALIGN(...);
        self.align(section_info.end_alignment_in_bytes);

        // _edata = .;
        self.set_symbol_to_current(ty.section_entry_end_symbol());

        // } >{MEMORY}
        self.output_section_end(section_info.target_memory.to_string());
    }

    fn add_stack_section_contents(&self) {
        let ty = SectionType::Stack;
        // _sstack =  .;
        self.set_symbol_to_current(ty.section_entry_start_symbol());
        // . = . + size;
        self.advance_location_counter(self.linker_config.stack_region_size());
        // _stack_top = .;
        self.set_symbol_to_current(stack_top_symbol());
        // _estack = .;
        self.set_symbol_to_current(ty.section_entry_end_symbol());
    }

    fn add_stack_section(&self, section_info: &Section) {
        if self.linker_config.is_stack_in_bss() {
            return;
        }

        let ty = &section_info.ty;

        // .stack (NOLOAD): ALIGN(...) {
        self.output_section_start(
            ty.section_entry_name(),
            true,
            section_info.start_alignment_in_bytes,
            section_info.load_address.clone(),
        );

        self.add_stack_section_contents();

        // . = ALIGN(...);
        self.align(section_info.end_alignment_in_bytes);

        // } >{MEMORY}
        self.output_section_end(section_info.target_memory.to_string());
    }

    fn add_bss_section(&self, section_info: &Section) {
        let ty = &section_info.ty;

        // .bss (NOLOAD): ALIGN(...) {
        self.output_section_start(
            ty.section_entry_name(),
            true,
            section_info.start_alignment_in_bytes,
            section_info.load_address.clone(),
        );

        // _sbss =  .;
        self.set_symbol_to_current(ty.section_entry_start_symbol());

        // *(.bss .bss.*)
        // *(.sbss .sbss.*)
        let default_sections = ty.default_sections();
        for input_section in default_sections {
            self.input_section(input_section, false);
        }

        if self.linker_config.is_stack_in_bss() {
            // . = ALIGN(...);
            self.align(self.linker_config.stack_in_bss_alignment());
            self.add_stack_section_contents();
        }

        // . = ALIGN(...);
        self.align(section_info.end_alignment_in_bytes);

        // _ebss = .;
        self.set_symbol_to_current(ty.section_entry_end_symbol());

        // } >{MEMORY}
        self.output_section_end(section_info.target_memory.to_string());
    }

    fn add_heap_section(&self, section_info: &Section) {
        let heap_size = self.linker_config.heap_size();

        if heap_size == 0 {
            return;
        }

        let ty = &section_info.ty;

        // .heap (NOLOAD): ALIGN(...) {
        self.output_section_start(
            ty.section_entry_name(),
            true,
            section_info.start_alignment_in_bytes,
            section_info.load_address.clone(),
        );

        // _sheap =  .;
        self.set_symbol_to_current(ty.section_entry_start_symbol());

        // . = . + heap_size;
        self.advance_location_counter(heap_size);

        // . = ALIGN(...);
        self.align(section_info.end_alignment_in_bytes);

        // _eheap = .;
        self.set_symbol_to_current(ty.section_entry_end_symbol());

        // } >{MEMORY}
        self.output_section_end(section_info.target_memory.to_string());
    }

    fn add_custom_section(&self, section_info: &Section, size: usize) {
        if size == 0 {
            return;
        }

        let ty = &section_info.ty;

        // If subsections are empty:
        // .{name} (NOLOAD): ALIGN(...) {
        // If subsections are not empty:
        // .{name} : ALIGN(...) {
        self.output_section_start(
            ty.section_entry_name(),
            section_info.subsections.is_empty(),
            section_info.start_alignment_in_bytes,
            section_info.load_address.clone(),
        );

        // _s{name} =  .;
        self.set_symbol_to_current(ty.section_entry_start_symbol());

        if section_info.subsections.is_empty() {
            // . = . + size;
            self.advance_location_counter(size);
        } else {
            // Handle all subsections
            self.add_subsection_information(section_info);
        }

        // . = ALIGN(...);
        self.align(section_info.end_alignment_in_bytes);

        // _e{name} = .;
        self.set_symbol_to_current(ty.section_entry_end_symbol());

        // } >{MEMORY}
        self.output_section_end(section_info.target_memory.to_string());
    }

    fn add_discard_section(&self) {
        let discard_sections = vec![
            ".eh_frame", // Discard exception handler frame
        ];

        self.add_sentence(LinkerSentence::DiscardSectionStart);

        for section in discard_sections {
            self.input_section(section, false);
        }

        self.add_sentence(LinkerSentence::DiscardSectionEnd);
    }

    fn sections(&self) {
        self.add_sentence(LinkerSentence::SectionsStart);

        for section in &self.linker_config.sections {
            match section.ty {
                SectionType::Text => self.add_text_section(section),
                SectionType::Rodata => self.add_rodata_section(section),
                SectionType::Data => self.add_data_section(section),
                SectionType::Bss => self.add_bss_section(section),
                SectionType::Stack => self.add_stack_section(section),
                SectionType::Heap => self.add_heap_section(section),
                SectionType::Custom(_, size) => self.add_custom_section(section, size),
            }
            self.add_section_to_memory(section);
        }

        self.add_discard_section();

        self.program_symbols();
        self.memory_symbols();
        self.add_sentence(LinkerSentence::SectionsEnd);
    }

    fn symbols(&self) {
        for symbol in &self.linker_config.symbols {
            self.add_symbol(symbol);
        }
    }

    fn add_symbol(&self, symbol: &Symbol) {
        self.add_sentence(LinkerSentence::Symbol(
            symbol.name.clone(),
            symbol.value.clone(),
        ));
    }

    fn asserts(&self) {
        for memory in &self.linker_config.memories {
            if memory.is_empty() {
                continue;
            }

            self.assert(
                format!(
                    "{:#} <= {:#}",
                    memory.start_symbol(),
                    memory.first_section_start_symbol()
                ),
                format!("{:#} underflow", memory.name),
            );
            self.assert(
                format!(
                    "{:#} >= {:#}",
                    memory.end_symbol(),
                    memory.last_section_end_symbol()
                ),
                format!("{:#} overflow", memory.name),
            );
        }
    }

    fn comment(&self, comment: &str) {
        self.add_sentence(LinkerSentence::Comment(comment.to_string()));
    }
}

fn write_linker_ld_file<'a>(
    dirpath: &Path,
    linker_config: &'a LinkerConfig<'a>,
) -> std::io::Result<()> {
    let filepath = dirpath.join("program.ld");
    let fw = FileWriter::new(filepath, BlockDelimiter::Parens);
    let linker = LinkerBuilder::new(linker_config);

    linker.output_arch(Arch::Riscv);
    linker.entry();
    linker.memory();
    linker.sections();
    linker.symbols();
    linker.asserts();
    linker.generate(&fw);
    fw.write()
}

fn region_start_fn_name(region_name: &str) -> String {
    format!("{region_name:#}_region_start")
}

fn region_end_fn_name(region_name: &str) -> String {
    format!("{region_name:#}_region_end")
}

fn region_size_fn_name(region_name: &str) -> String {
    format!("{region_name:#}_region_size")
}

fn define_get_addr_of(rust: &RustBuilder, fn_name: String, symbol: String) {
    rust.new_func_with_ret(fn_name, "usize".to_string());
    rust.addr_of(symbol);
    rust.end_func();
}

fn define_size_of(rust: &RustBuilder, region_name: &str) {
    rust.new_func_with_ret(region_size_fn_name(region_name), "usize".to_string());
    rust.sub(
        format!("{:#}()", region_end_fn_name(region_name)),
        format!("{:#}()", region_start_fn_name(region_name)),
    );
    rust.end_func();
}

fn define_stack_for_hart(rust: &RustBuilder, linker_config: &LinkerConfig) {
    let asm_fn_boot_id = GEN_FUNC_MAP.asm_fn(GeneratedFunc::BootId);

    rust.new_c_extern();
    rust.func_prototype(
        asm_fn_boot_id.clone(),
        Vec::new(),
        Some("usize".to_string()),
    );
    rust.end_extern();

    rust.new_func_with_ret("my_stack".to_string(), "(usize, usize)".to_string());
    rust.new_unsafe_block();
    rust.implicit_ret(format!(
        "({:#}() - {:#x} * ({:#}() + 1), {:#x})",
        region_end_fn_name(SectionType::Stack.name()),
        linker_config.hart_stack_size(),
        asm_fn_boot_id,
        linker_config.hart_stack_size()
    ));
    rust.end_unsafe_block();
    rust.end_func();
}

fn write_consts_rs_file(
    dirpath: &Path,
    linker_config: &LinkerConfig,
    root_fw: &FileWriter,
) -> std::io::Result<()> {
    let consts_rs_filename = "consts.rs";
    let filepath = dirpath.join(consts_rs_filename);
    let fw = FileWriter::new(filepath.clone(), BlockDelimiter::Parens);
    let rust = RustBuilder::new();

    rust.new_use("core::ptr::addr_of".to_string());

    rust.new_c_extern();

    let section_types = linker_config.section_types();

    for sty in &section_types {
        rust.static_def(sty.section_entry_start_symbol(), "usize".to_string());
        rust.static_def(sty.section_entry_end_symbol(), "usize".to_string());
    }

    for memory in &linker_config.memories {
        rust.static_def(memory.start_symbol(), "usize".to_string());
        rust.static_def(memory.end_symbol(), "usize".to_string());
    }

    rust.static_def(program_start_symbol(), "usize".to_string());
    rust.static_def(program_end_symbol(), "usize".to_string());

    rust.end_extern();

    for sty in &section_types {
        define_get_addr_of(
            &rust,
            region_start_fn_name(sty.name()),
            sty.section_entry_start_symbol(),
        );
        define_get_addr_of(
            &rust,
            region_end_fn_name(sty.name()),
            sty.section_entry_end_symbol(),
        );
        define_size_of(&rust, sty.name());
    }

    for memory in &linker_config.memories {
        define_get_addr_of(
            &rust,
            region_start_fn_name(memory.name()),
            memory.start_symbol(),
        );
        define_get_addr_of(
            &rust,
            region_end_fn_name(memory.name()),
            memory.end_symbol(),
        );
        define_size_of(&rust, memory.name());
    }

    // Provide the region occupied by the whole program.
    let program = "program";
    define_get_addr_of(&rust, region_start_fn_name(program), program_start_symbol());
    define_get_addr_of(&rust, region_end_fn_name(program), program_end_symbol());
    define_size_of(&rust, program);

    define_stack_for_hart(&rust, linker_config);

    rust.generate(&fw);

    add_module(root_fw, &filepath);
    fw.write()
}

pub fn write_linker_files<'a>(
    dirpath_name: &str,
    linker_config: &'a LinkerConfig<'a>,
    crate_type: CrateType,
) -> std::io::Result<()> {
    let dirpath = PathBuf::from(dirpath_name);
    let root_fw = create_root_rs_filewriter(&dirpath, crate_type);

    write_linker_ld_file(&dirpath, linker_config)?;
    write_consts_rs_file(&dirpath, linker_config, &root_fw)?;

    root_fw.write()
}
