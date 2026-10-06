// Copyright 2026 The Grim Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use grin_wallet_libwallet::Error;
use serde::{Serialize, de::DeserializeOwned};
use std::{
	fs::{self, File, OpenOptions},
	io::{Read, Write},
	path::Path,
};
use uuid::Uuid;

pub(super) const MAX_FILE: u64 = 4 * 1024 * 1024;

pub(super) fn fail(message: impl ToString) -> Error {
	Error::GenericError(message.to_string())
}

#[cfg(test)]
pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
	decode(File::open(path).map_err(fail)?)
}

pub(super) fn read_optional<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, Error> {
	match File::open(path) {
		Ok(file) => decode(file).map(Some),
		Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
		Err(error) => Err(fail(error)),
	}
}

fn decode<T: DeserializeOwned>(file: File) -> Result<T, Error> {
	let mut bytes = Vec::new();
	file.take(MAX_FILE + 1)
		.read_to_end(&mut bytes)
		.map_err(fail)?;
	if bytes.len() as u64 > MAX_FILE {
		return Err(fail("Swap file is too large"));
	}
	serde_json::from_slice(&bytes).map_err(fail)
}

pub(super) fn private_dir(path: &Path) -> Result<(), Error> {
	fs::create_dir_all(path).map_err(fail)?;
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(fail)?;
	}
	Ok(())
}

pub(super) fn write(path: &Path, bytes: &[u8]) -> Result<(), Error> {
	write_checkpointed(path, bytes, |_| {})
}

fn write_checkpointed(
	path: &Path,
	bytes: &[u8],
	mut checkpoint: impl FnMut(&str),
) -> Result<(), Error> {
	let parent = path
		.parent()
		.ok_or_else(|| fail("Missing swap directory"))?;
	let tmp = parent.join(format!(".{}.tmp", Uuid::new_v4()));
	let result = (|| {
		let mut options = OpenOptions::new();
		options.write(true).create_new(true);
		#[cfg(unix)]
		{
			use std::os::unix::fs::OpenOptionsExt;
			options.mode(0o600);
		}
		let mut file = options.open(&tmp).map_err(fail)?;
		file.write_all(bytes).map_err(fail)?;
		checkpoint("written");
		file.sync_all().map_err(fail)?;
		checkpoint("synced");
		fs::rename(&tmp, path).map_err(fail)?;
		checkpoint("renamed");
		#[cfg(unix)]
		File::open(parent)
			.and_then(|f| f.sync_all())
			.map_err(fail)?;
		checkpoint("durable");
		Ok(())
	})();
	if result.is_err() {
		let _ = fs::remove_file(tmp);
	}
	result
}

pub(super) fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
	write(path, &serde_json::to_vec(value).map_err(fail)?)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	#[ignore = "child process for interrupted_write"]
	fn crash_child() {
		let Ok(root) = std::env::var("GRIM_SWAP_CRASH_TEST_ROOT") else {
			return;
		};
		let stage = std::env::var("GRIM_SWAP_CRASH_TEST_STAGE").unwrap();
		let root = Path::new(&root);
		write_checkpointed(&root.join("session.json"), b"[2]", |point| {
			if point == stage {
				fs::write(root.join("ready"), b"ready").unwrap();
				loop {
					std::thread::park();
				}
			}
		})
		.unwrap();
	}

	#[test]
	fn interrupted_write() {
		use std::process::{Command, Stdio};
		use std::time::{Duration, Instant};
		for (stage, expected) in [
			("written", 1),
			("synced", 1),
			("renamed", 2),
			("durable", 2),
		] {
			let root = std::env::temp_dir().join(format!("grim-crash-{}", Uuid::new_v4()));
			private_dir(&root).unwrap();
			let path = root.join("session.json");
			save(&path, &vec![1u64]).unwrap();
			let mut child = Command::new(std::env::current_exe().unwrap())
				.args([
					"--ignored",
					"--exact",
					"wallet::swaps::storage::tests::crash_child",
				])
				.env("GRIM_SWAP_CRASH_TEST_ROOT", &root)
				.env("GRIM_SWAP_CRASH_TEST_STAGE", stage)
				.stdout(Stdio::null())
				.stderr(Stdio::null())
				.spawn()
				.unwrap();
			let deadline = Instant::now() + Duration::from_secs(60);
			while !root.join("ready").exists() && Instant::now() < deadline {
				if child.try_wait().unwrap().is_some() {
					break;
				}
				std::thread::sleep(Duration::from_millis(10));
			}
			let reached = root.join("ready").exists();
			let _ = child.kill();
			child.wait().unwrap();
			assert!(reached, "child did not reach {stage}");
			// A killed writer may leave a temporary file. The loader must still
			// read only the complete old or new session at its committed path.
			assert_eq!(
				read_optional::<Vec<u64>>(&path).unwrap(),
				Some(vec![expected])
			);
			save(&path, &vec![3u64]).unwrap();
			assert_eq!(read::<Vec<u64>>(&path).unwrap(), vec![3]);
			fs::remove_dir_all(root).unwrap();
		}
	}
}
