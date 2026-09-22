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

use super::fail;
use grin_wallet_config::types::BitcoinConfig;
use grin_wallet_impls::swap::adapters::bitcoin::{
	Node as BitcoinNode,
	types::{Address, Network},
};
use grin_wallet_libwallet::Error;
use std::{path::PathBuf, str::FromStr};

pub struct NetworkInfo {
	pub name: &'static str,
	pub label: &'static str,
	pub path: Option<&'static str>,
	pub port: u16,
}

pub const NETWORKS: &[NetworkInfo] = &[
	NetworkInfo {
		name: "testnet4",
		label: "Testnet 4",
		path: Some("testnet4"),
		port: 48332,
	},
	NetworkInfo {
		name: "testnet",
		label: "Testnet 3",
		path: Some("testnet"),
		port: 18332,
	},
	NetworkInfo {
		name: "signet",
		label: "Signet",
		path: Some("signet"),
		port: 38332,
	},
	NetworkInfo {
		name: "regtest",
		label: "Regtest",
		path: None,
		port: 18443,
	},
];

pub fn server(network: &str, remote: bool) -> Option<String> {
	let info = NETWORKS.iter().find(|info| info.name == network)?;
	if remote {
		Some(format!("https://mempool.space/{}/api", info.path?))
	} else {
		Some(format!("http://127.0.0.1:{}", info.port))
	}
}

pub fn explorer_url(network: &str, kind: &str, value: &str) -> Option<String> {
	let path = NETWORKS.iter().find(|info| info.name == network)?.path?;
	Some(format!("https://mempool.space/{path}/{kind}/{value}"))
}

pub fn validate_address(network: &str, address: &str) -> bool {
	Network::from_str(network).ok().is_some_and(|network| {
		Address::from_str(address.trim())
			.ok()
			.is_some_and(|address| address.require_network(network).is_ok())
	})
}

pub(super) fn default_bitcoin_config() -> BitcoinConfig {
	BitcoinConfig {
		url: "https://mempool.space/testnet4/api".into(),
		cookie: PathBuf::new(),
		network: "testnet4".into(),
		proxy: None,
	}
}

pub(super) fn node(config: &BitcoinConfig) -> Result<BitcoinNode, Error> {
	let network = test_network(&config.network)?;
	BitcoinNode::with_proxy(
		&config.url,
		config.cookie.clone(),
		network,
		config.proxy.as_deref(),
	)
}

pub(super) fn test_network(name: &str) -> Result<Network, Error> {
	let network = Network::from_str(name).map_err(fail)?;
	if network == Network::Bitcoin {
		return Err(fail("Choose a Bitcoin test network"));
	}
	Ok(network)
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn networks() {
		assert_eq!(
			server("testnet4", true).as_deref(),
			Some("https://mempool.space/testnet4/api")
		);
		assert_eq!(
			server("regtest", false).as_deref(),
			Some("http://127.0.0.1:18443")
		);
		assert!(server("regtest", true).is_none());
		assert!(server("unknown", false).is_none());
		let mut config = default_bitcoin_config();
		config.network = "bitcoin".into();
		assert!(node(&config).is_err());
	}
}
