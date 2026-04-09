//! CPIO "newc" format writer for assembling initramfs archives.

use std::io::{self, Write};

pub(super) struct CpioWriter<W: Write> {
	inner: W,
	ino: u32,
	offset: usize,
}

impl<W: Write> CpioWriter<W> {
	pub(super) const fn new(writer: W) -> Self {
		Self {
			inner: writer,
			ino: 1,
			offset: 0,
		}
	}

	fn align4(&mut self) -> io::Result<()> {
		let pad = (4 - (self.offset % 4)) % 4;
		if pad > 0 {
			self.inner.write_all(&vec![0u8; pad])?;
			self.offset += pad;
		}
		Ok(())
	}

	fn write_entry(
		&mut self,
		name: &str,
		data: &[u8],
		mode: u32,
	) -> io::Result<()> {
		let namesize = name.len() + 1;
		let filesize = data.len();

		let header = format!(
			"070701{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:08X}{:\
			 08X}{:08X}{:08X}",
			self.ino,
			mode,
			0u32,
			0u32, // ino, mode, uid, gid
			1u32,
			0u32,
			filesize,
			0u32, // nlink, mtime, filesize, devmajor
			0u32,
			0u32,
			0u32,
			namesize,
			0u32, // devminor, rdevmaj, rdevmin, namesize, check
		);
		debug_assert_eq!(header.len(), 110);

		self.inner.write_all(header.as_bytes())?;
		self.offset += 110;

		self.inner.write_all(name.as_bytes())?;
		self.inner.write_all(&[0])?;
		self.offset += namesize;
		self.align4()?;

		self.inner.write_all(data)?;
		self.offset += filesize;
		self.align4()?;

		self.ino += 1;
		Ok(())
	}

	pub(super) fn add_file(
		&mut self,
		path: &str,
		data: &[u8],
		executable: bool,
	) -> io::Result<()> {
		self.write_entry(path, data, if executable { 0o100_755 } else { 0o100_644 })
	}

	pub(super) fn add_dir(&mut self, path: &str) -> io::Result<()> {
		self.write_entry(path, &[], 0o40755)
	}

	pub(super) fn add_symlink(
		&mut self,
		path: &str,
		target: &str,
	) -> io::Result<()> {
		self.write_entry(path, target.as_bytes(), 0o120_777)
	}

	pub(super) fn finish(mut self) -> io::Result<W> {
		self.write_entry("TRAILER!!!", &[], 0)?;
		let pad = (512 - (self.offset % 512)) % 512;
		if pad > 0 {
			self.inner.write_all(&vec![0u8; pad])?;
		}
		Ok(self.inner)
	}
}
