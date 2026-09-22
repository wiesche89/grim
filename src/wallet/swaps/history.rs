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

use super::storage::{fail, read_optional};
use grin_keychain::Keychain;
use grin_util::{ToHex, secp::SecretKey};
use grin_wallet_libwallet::{
	Error, NodeClient, TxLogEntry, WalletBackend,
	swap::records::{Record, TxKind},
};
use serde::Deserialize;
use std::{fs, path::Path};
use uuid::Uuid;

pub fn visible(tx: &TxLogEntry) -> bool {
	match tx.swap.as_ref().map(|s| s.kind) {
		Some(TxKind::Internal) => false,
		Some(TxKind::Recovery) => tx.confirmed,
		_ => true,
	}
}

// Import GRIM's existing session files before the first wallet refresh
pub fn import_records<C: NodeClient, K: Keychain>(
	wallet: &mut WalletBackend<C, K>,
	mask: Option<&SecretKey>,
	root: &Path,
) -> Result<(), Error> {
	#[derive(Deserialize)]
	struct Session {
		preparation: Record,
	}
	let parent = wallet.parent_key_id();
	let result = (|| {
		let accounts = wallet.acct_path_iter()?.collect::<Vec<_>>();
		for account in accounts {
			let files = match fs::read_dir(root.join(account.label.as_bytes().to_hex())) {
				Ok(files) => files,
				Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
				Err(e) => return Err(fail(e)),
			};
			wallet.set_parent_key_id(account.path);
			for file in files {
				let path = file.map_err(fail)?.path();
				let name = path.file_stem().and_then(|s| s.to_str());
				if path.extension().and_then(|s| s.to_str()) != Some("json")
					|| !name.is_some_and(|s| s == "session" || Uuid::parse_str(s).is_ok())
				{
					continue;
				}
				if let Some(session) = read_optional::<Session>(&path)? {
					if session.preparation.slates.contains_key("fund") {
						wallet.register_swap(mask, &session.preparation)?;
					}
				}
			}
		}
		Ok(())
	})();
	wallet.set_parent_key_id(parent);
	result
}
