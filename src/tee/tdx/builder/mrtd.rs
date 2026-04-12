//! TDX measurement precomputation (MRTD, RTMR\[1\], RTMR\[2\]).

use sha2::{Digest, Sha384};

const PAGE_SIZE: usize = 4096;
const CHUNK_SIZE: usize = 256;
const CHUNKS_PER_PAGE: usize = PAGE_SIZE / CHUNK_SIZE;
const SHA384_LEN: usize = 48;

// e47a6535-984a-4798-865e-4685a7bf8ec2 (little-endian)
const TDVF_METADATA_GUID: [u8; 16] = [
	0x35, 0x65, 0x7a, 0xe4, 0x4a, 0x98, 0x98, 0x47, 0x86, 0x5e, 0x46, 0x85, 0xa7,
	0xbf, 0x8e, 0xc2,
];

// 96b582de-1fb2-45f7-baea-a366c55a082d (little-endian)
const TABLE_FOOTER_GUID: [u8; 16] = [
	0xde, 0x82, 0xb5, 0x96, 0xb2, 0x1f, 0xf7, 0x45, 0xba, 0xea, 0xa3, 0x66, 0xc5,
	0x5a, 0x08, 0x2d,
];

#[derive(Debug)]
struct TdvfSection {
	data_offset: u32,
	raw_data_size: u32,
	memory_address: u64,
	memory_data_size: u64,
	section_type: u32,
	attributes: u32,
}

const SECTION_TYPE_BFV: u32 = 0;
const SECTION_TYPE_CFV: u32 = 1;
const SECTION_TYPE_TD_HOB: u32 = 2;
const SECTION_TYPE_TEMP_MEM: u32 = 3;
const ATTR_MR_EXTEND: u32 = 0x0000_0001;
const ATTR_PAGE_AUG: u32 = 0x0000_0002;

/// Locate the start of the TDVF metadata descriptor in the
/// OVMF image. Returns the byte offset (from the start of
/// `ovmf`) where the descriptor begins.
///
/// Two discovery methods are tried in order:
///  1. **GUID table** (modern OVMF builds): a footer GUID at `file_end − 0x30`
///     points to a table of entries; the TDVF metadata offset entry stores a
///     negative offset from the end of the file.
///  2. **Legacy pointer** (deprecated): a 32-bit *absolute* offset stored at
///     `file_end − 0x20`.
fn find_tdvf_descriptor_offset(ovmf: &[u8]) -> Result<usize, String> {
	let len = ovmf.len();

	// --- Method 1: GUID table ---
	if len >= 0x32 {
		let footer_start = len - 0x30;
		let footer_guid = &ovmf[footer_start..footer_start + 16];

		if footer_guid == TABLE_FOOTER_GUID {
			eprintln!("  [tdvf] Found GUID table footer");

			let table_size =
				u16::from_le_bytes(ovmf[len - 0x32..len - 0x30].try_into().unwrap())
					as usize;

			let table_start = len - 0x20 - table_size;
			let table = &ovmf[table_start..table_start + table_size];

			// Walk backward: skip footer GUID (16) + size (2)
			let mut offset = table_size.saturating_sub(18);
			while offset >= 18 {
				let entry_guid = &table[offset - 16..offset];
				let entry_size = u16::from_le_bytes(
					table[offset - 18..offset - 16].try_into().unwrap(),
				) as usize;

				if entry_size == 0 {
					break;
				}
				offset -= entry_size;

				if entry_guid == TDVF_METADATA_GUID && entry_size == 22 {
					let desc_off =
						u32::from_le_bytes(table[offset..offset + 4].try_into().unwrap())
							as usize;
					eprintln!("  [tdvf] GUID table: descriptor at end-0x{desc_off:x}");
					return Ok(len - desc_off);
				}
			}
			eprintln!(
				"  [tdvf] GUID table present but TDVF metadata entry not found, \
				 trying legacy"
			);
		}
	}

	// --- Method 2: legacy absolute offset at file_end - 0x20 ---
	if len >= 0x28 {
		let descriptor_offset =
			u32::from_le_bytes(ovmf[len - 0x20..len - 0x1c].try_into().unwrap())
				as usize;

		if descriptor_offset > 0 && descriptor_offset < len {
			eprintln!(
				"  [tdvf] Legacy pointer: descriptor at 0x{descriptor_offset:x}"
			);
			return Ok(descriptor_offset);
		}
	}

	Err(format!(
		"Could not locate TDVF metadata (no GUID table entry, legacy offset is \
		 0x0, file size: 0x{len:x})"
	))
}

const TDVF_SIGNATURE: &[u8; 4] = b"TDVF";
const TDVF_HEADER_SIZE: usize = 16; // signature(4) + length(4) + version(4) + num_sections(4)

fn parse_tdvf_sections(ovmf: &[u8]) -> Result<Vec<TdvfSection>, String> {
	let desc_start = find_tdvf_descriptor_offset(ovmf)?;
	let desc = &ovmf[desc_start..];

	if desc.len() < TDVF_HEADER_SIZE {
		return Err("TDVF descriptor region too small".into());
	}

	let signature = &desc[0..4];
	if signature != TDVF_SIGNATURE {
		return Err(format!(
			"TDVF descriptor signature mismatch: expected \"TDVF\", got \
			 {signature:02x?}"
		));
	}

	let _length = u32::from_le_bytes(desc[4..8].try_into().unwrap());
	let version = u32::from_le_bytes(desc[8..12].try_into().unwrap());
	let num_sections = u32::from_le_bytes(desc[12..16].try_into().unwrap());

	eprintln!("  [tdvf] version={version}, sections={num_sections}");

	let mut sections = Vec::new();
	for i in 0..num_sections as usize {
		let base = TDVF_HEADER_SIZE + i * 32;
		if base + 32 > desc.len() {
			return Err(format!("TDVF section {i} extends past descriptor"));
		}
		let s = &desc[base..base + 32];
		let section = TdvfSection {
			data_offset: u32::from_le_bytes(s[0..4].try_into().unwrap()),
			raw_data_size: u32::from_le_bytes(s[4..8].try_into().unwrap()),
			memory_address: u64::from_le_bytes(s[8..16].try_into().unwrap()),
			memory_data_size: u64::from_le_bytes(s[16..24].try_into().unwrap()),
			section_type: u32::from_le_bytes(s[24..28].try_into().unwrap()),
			attributes: u32::from_le_bytes(s[28..32].try_into().unwrap()),
		};
		let type_name = match section.section_type {
			SECTION_TYPE_BFV => "BFV",
			SECTION_TYPE_CFV => "CFV",
			SECTION_TYPE_TD_HOB => "TD_HOB",
			SECTION_TYPE_TEMP_MEM => "TEMP_MEM",
			_ => "UNKNOWN",
		};
		let mr = if section.attributes & ATTR_MR_EXTEND != 0 {
			" [EXTENDMR]"
		} else {
			""
		};
		let aug = if section.attributes & ATTR_PAGE_AUG != 0 {
			" [PAGE_AUG]"
		} else {
			""
		};
		eprintln!(
			"  [tdvf]   section[{i}]: type={type_name} gpa=0x{:x} size=0x{:x} \
			 raw_offset=0x{:x} raw_size=0x{:x}{mr}{aug}",
			section.memory_address,
			section.memory_data_size,
			section.data_offset,
			section.raw_data_size,
		);
		sections.push(section);
	}

	Ok(sections)
}

pub(super) fn compute_mrtd(ovmf: &[u8]) -> Result<[u8; SHA384_LEN], String> {
	let sections = parse_tdvf_sections(ovmf)?;

	// The TDX module maintains a running SHA-384 context across
	// all TDH.MEM.PAGE.ADD and TDH.MR.EXTEND calls, then
	// finalizes it on TDH.MR.FINALIZE.  This is equivalent to
	// SHA384(concat of all 128-byte headers and data chunks).
	let mut hasher = Sha384::new();

	for section in &sections {
		let extend_mr = section.attributes & ATTR_MR_EXTEND != 0;
		let page_aug = section.attributes & ATTR_PAGE_AUG != 0;

		// PAGE_AUG pages are added lazily via TDH.MEM.PAGE.AUG
		// which does NOT update MRTD.
		if page_aug {
			continue;
		}

		let gpa_base = section.memory_address;
		let mem_size = section.memory_data_size as usize;
		let raw_offset = section.data_offset as usize;
		let raw_size = section.raw_data_size as usize;

		let section_data = if raw_size > 0 && raw_offset + raw_size <= ovmf.len() {
			&ovmf[raw_offset..raw_offset + raw_size]
		} else {
			&[] as &[u8]
		};

		let num_pages = mem_size.div_ceil(PAGE_SIZE);

		eprintln!(
			"  [mrtd] section GPA 0x{gpa_base:x}, {num_pages} pages{}",
			if extend_mr { " [MR.EXTEND]" } else { "" },
		);

		// Phase 1: TDH.MEM.PAGE.ADD for every page in this section
		for page_idx in 0..num_pages {
			let page_gpa = gpa_base + (page_idx as u64) * (PAGE_SIZE as u64);

			let mut add_buf = [0u8; 128];
			add_buf[..12].copy_from_slice(b"MEM.PAGE.ADD");
			add_buf[16..24].copy_from_slice(&page_gpa.to_le_bytes());

			hasher.update(add_buf);
		}

		// Phase 2: TDH.MR.EXTEND for every 256-byte chunk (only
		// for sections with the MR_EXTEND attribute)
		if extend_mr {
			for page_idx in 0..num_pages {
				let page_gpa = gpa_base + (page_idx as u64) * (PAGE_SIZE as u64);

				let page_data_start = page_idx * PAGE_SIZE;
				let mut page_data = [0u8; PAGE_SIZE];
				if page_data_start < section_data.len() {
					let available = section_data.len() - page_data_start;
					let copy_len = available.min(PAGE_SIZE);
					page_data[..copy_len].copy_from_slice(
						&section_data[page_data_start..page_data_start + copy_len],
					);
				}

				for chunk_idx in 0..CHUNKS_PER_PAGE {
					let chunk_gpa = page_gpa + (chunk_idx as u64) * (CHUNK_SIZE as u64);

					let mut ext_header = [0u8; 128];
					ext_header[..9].copy_from_slice(b"MR.EXTEND");
					ext_header[16..24].copy_from_slice(&chunk_gpa.to_le_bytes());

					let chunk_start = chunk_idx * CHUNK_SIZE;
					let chunk_data = &page_data[chunk_start..chunk_start + CHUNK_SIZE];

					hasher.update(ext_header);
					hasher.update(chunk_data);
				}
			}
		}
	}

	let mut mrtd = [0u8; SHA384_LEN];
	mrtd.copy_from_slice(&hasher.finalize());

	eprintln!("  [mrtd] MRTD = {}", hex::encode(mrtd));

	Ok(mrtd)
}

/// Compute the expected RTMR[2] register value.
///
/// RTMR[2] receives exactly two events during TDX boot:
///  1. Kernel command line (UTF-16LE with null terminator)
///  2. Initrd (raw file bytes, typically gzipped CPIO)
///
/// Each event extends the register:
///   `RTMR[2] = SHA384(RTMR[2] || SHA384(preimage))`
///
/// For non-UKI boots (separate -kernel/-initrd), QEMU appends
/// `" initrd=initrd"` to the measured command line when an initrd
/// is present.
pub(super) fn compute_rtmr2(cmdline: &str, initrd: &[u8]) -> [u8; SHA384_LEN] {
	let mut rtmr2 = [0u8; SHA384_LEN];

	// Event 1: kernel command line
	// QEMU appends " initrd=initrd" when an initrd is provided.
	let measured_cmdline = if initrd.is_empty() {
		format!("{cmdline}\0")
	} else {
		format!("{cmdline} initrd=initrd\0")
	};

	// Encode as UTF-16LE
	let cmdline_utf16: Vec<u8> = measured_cmdline
		.encode_utf16()
		.flat_map(|c| c.to_le_bytes())
		.collect();

	let cmdline_digest = Sha384::digest(&cmdline_utf16);
	let mut extend_input = Vec::with_capacity(SHA384_LEN * 2);
	extend_input.extend_from_slice(&rtmr2);
	extend_input.extend_from_slice(&cmdline_digest);
	rtmr2.copy_from_slice(&Sha384::digest(&extend_input));

	eprintln!(
		"  [rtmr2] event 1: cmdline ({} UTF-16LE bytes)",
		cmdline_utf16.len(),
	);

	// Event 2: initrd (raw file bytes as loaded by QEMU)
	if !initrd.is_empty() {
		let initrd_digest = Sha384::digest(initrd);
		extend_input.clear();
		extend_input.extend_from_slice(&rtmr2);
		extend_input.extend_from_slice(&initrd_digest);
		rtmr2.copy_from_slice(&Sha384::digest(&extend_input));

		eprintln!(
			"  [rtmr2] event 2: initrd ({:.1} MB)",
			initrd.len() as f64 / 1_048_576.0,
		);
	}

	let hex = hex::encode(rtmr2);
	eprintln!("  [rtmr2] RTMR[2] = {hex}");

	rtmr2
}

// -----------------------------------------------------------------
// RTMR[1] — kernel boot measurement
// -----------------------------------------------------------------

/// QEMU memory layout: determines the below-4G RAM size which
/// affects where the initrd is loaded and thus the kernel setup
/// header patches.
struct MemoryLayout {
	below_4g: u64,
}

/// Size of QEMU's ACPI data region.
const ACPI_DATA_SIZE: u64 = 0x20000 + 0x8000;

/// Parse a QEMU `-m` memory string into bytes.
/// Accepts "4G", "4096M", or bare MiB number.
fn parse_memory_bytes(s: &str) -> u64 {
	let s = s.trim();
	s.strip_suffix('G')
		.or_else(|| s.strip_suffix('g'))
		.map_or_else(
			|| {
				s.strip_suffix('M')
					.or_else(|| s.strip_suffix('m'))
					.map_or_else(
						|| s.parse::<u64>().unwrap() * 1024 * 1024,
						|n| n.trim().parse::<u64>().unwrap() * 1024 * 1024,
					)
			},
			|n| n.trim().parse::<u64>().unwrap() * 1024 * 1024 * 1024,
		)
}

/// Compute the QEMU PC memory layout from total RAM bytes.
const fn memory_layout(total: u64) -> MemoryLayout {
	let lowmem = if total >= 0xb000_0000 {
		0x8000_0000
	} else {
		0xb000_0000
	};
	MemoryLayout {
		below_4g: if total >= lowmem { lowmem } else { total },
	}
}

/// Replicate QEMU's `x86_load_linux` kernel setup-header patches.
///
/// QEMU modifies the kernel image in memory before OVMF measures
/// it, so the PE Authenticode hash must be computed over the
/// patched bytes.
fn patch_kernel_setup(
	kernel: &mut [u8],
	initrd_len: usize,
	layout: &MemoryLayout,
) {
	let magic = &kernel[0x202..0x206];
	let protocol = if magic == b"HdrS" {
		u16::from_le_bytes(kernel[0x206..0x208].try_into().unwrap())
	} else {
		0
	};

	let (real_addr, cmdline_addr): (u32, u32) =
		if protocol >= 0x202 && (kernel[0x211] & 0x01) != 0 {
			(0x10000, 0x20000)
		} else {
			(0x90000, 0x9a000_u32.wrapping_sub(32))
		};

	// Determine maximum initrd address
	let mut initrd_max: u64 = if protocol >= 0x20c
		&& (u16::from_le_bytes(kernel[0x236..0x238].try_into().unwrap()) & 2) != 0
	{
		0xffff_ffff
	} else if protocol >= 0x203 {
		u64::from(u32::from_le_bytes(kernel[0x22c..0x230].try_into().unwrap()))
	} else {
		0x37ff_ffff
	};

	let mem_cap = layout.below_4g - ACPI_DATA_SIZE;
	if initrd_max >= mem_cap {
		initrd_max = mem_cap - 1;
	}

	// Patch cmdline pointer
	if protocol >= 0x202 {
		kernel[0x228..0x22c].copy_from_slice(&cmdline_addr.to_le_bytes());
	} else {
		kernel[0x20..0x22].copy_from_slice(&0xa33f_u16.to_le_bytes());
		kernel[0x22..0x24].copy_from_slice(
			&(cmdline_addr.wrapping_sub(real_addr) as u16).to_le_bytes(),
		);
	}

	// type_of_loader
	if protocol >= 0x200 {
		kernel[0x210] = 0xb0;
	}

	// loadflags + heap_end_ptr
	if protocol >= 0x201 {
		kernel[0x211] |= 0x80;
		kernel[0x224..0x226].copy_from_slice(
			&((cmdline_addr.wrapping_sub(real_addr).wrapping_sub(0x200)) as u16)
				.to_le_bytes(),
		);
	}

	// initrd address + size
	if initrd_len > 0 {
		assert!(
			(initrd_len as u64) < initrd_max,
			"initrd too large for memory layout"
		);
		let initrd_addr = ((initrd_max - initrd_len as u64) & !4095) as u32;
		kernel[0x218..0x21c].copy_from_slice(&initrd_addr.to_le_bytes());
		kernel[0x21c..0x220].copy_from_slice(&(initrd_len as u32).to_le_bytes());
	}
}

/// Compute the PE Authenticode hash preimage.
///
/// This concatenates the PE file contents in hash order: headers
/// (excluding the checksum field and certificate directory entry),
/// then section bodies sorted by raw data pointer, then any
/// trailing data minus the certificate blob.
fn pe_hash_preimage(data: &[u8]) -> Vec<u8> {
	// Section bodies sorted by raw-data pointer
	struct SecInfo {
		ptr: usize,
		size: usize,
	}

	let e_lfanew =
		u32::from_le_bytes(data[0x3c..0x40].try_into().unwrap()) as usize;

	let file_header_offset = e_lfanew + 4;
	let num_sections = u16::from_le_bytes(
		data[file_header_offset + 2..file_header_offset + 4]
			.try_into()
			.unwrap(),
	) as usize;
	let size_of_optional_header = u16::from_le_bytes(
		data[file_header_offset + 16..file_header_offset + 18]
			.try_into()
			.unwrap(),
	) as usize;
	let opt = file_header_offset + 20;

	let magic = u16::from_le_bytes(data[opt..opt + 2].try_into().unwrap());
	let fixed_opt_size: usize = match magic {
		0x10b => 96,  // PE32
		0x20b => 112, // PE32+
		_ => panic!("Unknown PE optional-header magic: {magic:#x}"),
	};

	let size_of_headers =
		u32::from_le_bytes(data[opt + 60..opt + 64].try_into().unwrap()) as usize;
	let num_rva = u32::from_le_bytes(
		data[opt + fixed_opt_size - 4..opt + fixed_opt_size]
			.try_into()
			.unwrap(),
	) as usize;

	let checksum_off = opt + 0x40;
	let security_dir_idx = 4;

	let mut parts: Vec<&[u8]> = Vec::new();

	// Everything before the checksum field
	parts.push(&data[..checksum_off]);

	// After checksum, skip cert-dir entry if present
	let after_checksum = checksum_off + 4;
	if num_rva <= security_dir_idx {
		parts.push(&data[after_checksum..size_of_headers]);
	} else {
		let cert_dir_off = opt + fixed_opt_size + security_dir_idx * 8;
		parts.push(&data[after_checksum..cert_dir_off]);
		let after_cert = cert_dir_off + 8;
		parts.push(&data[after_cert..size_of_headers]);
	}

	let sh_start = opt + size_of_optional_header;
	let mut sections: Vec<SecInfo> = (0..num_sections)
		.filter_map(|i| {
			let sh = sh_start + i * 40;
			let raw_size =
				u32::from_le_bytes(data[sh + 16..sh + 20].try_into().unwrap()) as usize;
			let raw_ptr =
				u32::from_le_bytes(data[sh + 20..sh + 24].try_into().unwrap()) as usize;
			if raw_size > 0 {
				Some(SecInfo {
					ptr: raw_ptr,
					size: raw_size,
				})
			} else {
				None
			}
		})
		.collect();
	sections.sort_by_key(|s| s.ptr);

	let mut sum_hashed = size_of_headers;
	for s in &sections {
		parts.push(&data[s.ptr..s.ptr + s.size]);
		sum_hashed += s.size;
	}

	// Trailing data beyond sections, minus cert blob
	if data.len() > sum_hashed {
		let mut cert_size = 0usize;
		if num_rva > security_dir_idx {
			let off = opt + fixed_opt_size + security_dir_idx * 8 + 4;
			if off + 4 <= data.len() {
				cert_size =
					u32::from_le_bytes(data[off..off + 4].try_into().unwrap()) as usize;
			}
		}
		if data.len() > sum_hashed + cert_size {
			parts.push(&data[sum_hashed..data.len() - cert_size]);
		}
	}

	let total_len: usize = parts.iter().map(|p| p.len()).sum();
	let mut result = Vec::with_capacity(total_len);
	for part in parts {
		result.extend_from_slice(part);
	}
	result
}

/// Precompute RTMR[1] for a non-UKI TDX boot.
///
/// RTMR[1] receives four events:
///  1. Kernel PE Authenticode hash (after QEMU setup-header patches)
///  2. "Calling EFI Application from Boot Option"
///  3. "Exit Boot Services Invocation"
///  4. "Exit Boot Services Returned with Success"
///
/// The kernel is cloned and patched exactly as QEMU does before
/// OVMF measures it: cmdline pointer, initrd address/size,
/// `type_of_loader`, `loadflags`, and `heap_end_ptr`.
pub(super) fn compute_rtmr1(
	kernel: &[u8],
	initrd: &[u8],
	memory: &str,
) -> [u8; SHA384_LEN] {
	let mut rtmr1 = [0u8; SHA384_LEN];
	let mut extend = Vec::with_capacity(SHA384_LEN * 2);

	// Clone kernel and apply QEMU setup-header patches
	let mut patched = kernel.to_vec();
	let layout = memory_layout(parse_memory_bytes(memory));
	patch_kernel_setup(&mut patched, initrd.len(), &layout);

	// Event 1: kernel PE Authenticode hash
	let preimage = pe_hash_preimage(&patched);
	let digest = Sha384::digest(&preimage);
	extend.extend_from_slice(&rtmr1);
	extend.extend_from_slice(&digest);
	rtmr1.copy_from_slice(&Sha384::digest(&extend));
	eprintln!(
		"  [rtmr1] event 1: kernel PE hash ({} bytes preimage)",
		preimage.len(),
	);

	// Events 2-4: fixed EFI action strings
	#[allow(clippy::items_after_statements)]
	const EFI_ACTIONS: [&[u8]; 3] = [
		b"Calling EFI Application from Boot Option",
		b"Exit Boot Services Invocation",
		b"Exit Boot Services Returned with Success",
	];

	for (i, action) in EFI_ACTIONS.iter().enumerate() {
		let digest = Sha384::digest(action);
		extend.clear();
		extend.extend_from_slice(&rtmr1);
		extend.extend_from_slice(&digest);
		rtmr1.copy_from_slice(&Sha384::digest(&extend));
		eprintln!(
			"  [rtmr1] event {}: {:?}",
			i + 2,
			std::str::from_utf8(action).unwrap(),
		);
	}

	let hex = hex::encode(rtmr1);
	eprintln!("  [rtmr1] RTMR[1] = {hex}");

	rtmr1
}
