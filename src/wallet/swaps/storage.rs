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
		file.sync_all().map_err(fail)?;
		fs::rename(&tmp, path).map_err(fail)?;
		#[cfg(unix)]
		File::open(parent)
			.and_then(|f| f.sync_all())
			.map_err(fail)?;
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
