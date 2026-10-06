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

use super::storage::*;
use super::*;
use grin_wallet_api::{Foreign, Owner};
use grin_wallet_libwallet::swap::sas::Terms;
use std::{fs, path::Path};
#[test]
fn storage() {
	let root = std::env::temp_dir().join(format!("grim-swaps-{}", Uuid::new_v4()));
	private_dir(&root).unwrap();
	let path = root.join("state.json");
	save(&path, &vec![1u64, 2]).unwrap();
	save(&path, &vec![3u64]).unwrap();
	assert_eq!(read::<Vec<u64>>(&path).unwrap(), vec![3]);
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt;
		assert_eq!(
			fs::metadata(&path).unwrap().permissions().mode() & 0o777,
			0o600
		);
	}
	fs::write(&path, vec![b' '; MAX_FILE as usize + 1]).unwrap();
	assert!(read::<serde_json::Value>(&path).is_err());
	fs::remove_dir_all(root).unwrap();
}

#[rustfmt::skip]
#[macro_use]
#[path = "../../../wallet/controller/tests/common/mod.rs"]
mod common;

use common::bitcoin;

#[test]
#[ignore = "requires an isolated funded Bitcoin Core regtest wallet"]
fn slatepacks() -> Result<(), Error> {
	use grin_wallet_impls::test_framework::{self, LocalWalletClient};
	let dir: &'static str = Box::leak(
		std::env::temp_dir()
			.join(format!("grim-exchange-{}", Uuid::new_v4()))
			.to_string_lossy()
			.into_owned()
			.into_boxed_str(),
	);
	common::setup(dir);
	private_dir(Path::new(dir))?;
	let mut proxy = common::create_wallet_proxy(dir);
	let chain = proxy.chain.clone();
	let running = proxy.running.clone();
	create_wallet_and_add!(client1, wallet1, mask1, dir, "a", None, &mut proxy, true);
	create_wallet_and_add!(client2, wallet2, mask2, dir, "b", None, &mut proxy, true);
	let proxy_thread = thread::spawn(move || proxy.run().unwrap());
	let am = mask1.as_ref();
	let bm = mask2.as_ref();
	test_framework::award_blocks_to_wallet(&chain, wallet1.clone(), am, 10, false)?;
	let config = BitcoinConfig {
		proxy: None,
		url: std::env::var("GRIN_SWAP_RPC").unwrap(),
		cookie: std::env::var("GRIN_SWAP_COOKIE").unwrap().into(),
		network: "regtest".into(),
	};
	let path = Path::new(dir).join("bitcoin.toml");
	let a = Owner::new(wallet1.clone(), None, path.clone()).with_bitcoin_config(config.clone());
	let b = Owner::new(wallet2.clone(), None, path.clone()).with_bitcoin_config(config.clone());
	let fa = Foreign::new(wallet1.clone(), path.clone(), mask1.clone(), None, false);
	let fb = Foreign::new(wallet2.clone(), path, mask2.clone(), None, false);
	let root = Path::new(dir);
	let worker = |name: &str| {
		let root = root.join(name).join("swaps");
		private_dir(&root).unwrap();
		Worker {
			timing: Timing::default(),
			root,
			session: None,
			reply: None,
			updated: None,
		}
	};
	let mut seller = worker("a");
	seller.timing = Timing {
		revoke: 1441,
		refund: 1562,
		timeout: 1683,
		confirmations: 3,
		bitcoin_confirmations: 3,
		margin: 40,
	};
	save(&seller.root.join("timing.json"), &seller.timing)?;
	seller.timing = read(&seller.root.join("timing.json"))?;
	let mut buyer = worker("b");
	buyer.timing = seller.timing;
	let address = bitcoin(&["getnewaddress"]).as_str().unwrap().to_owned();
	let offer = || Command::Create {
		grin: 5_012_500_000,
		bitcoin: 100_000,
		address: address.clone(),
	};
	let balance = a
		.retrieve_summary_info(am, true, 2)?
		.1
		.amount_currently_spendable;
	seller.advance(&a, &fa, am, &config, Some(offer()))?;
	seller.advance(&a, &fa, am, &config, Some(Command::Abort))?;
	assert!(seller.view().unwrap().cancelled);
	assert_eq!(
		a.retrieve_summary_info(am, true, 2)?
			.1
			.amount_currently_spendable,
		balance
	);
	seller.advance(&a, &fa, am, &config, Some(Command::Archive))?;
	seller.advance(&a, &fa, am, &config, Some(offer()))?;
	let terms = seller.view().unwrap().proposal.terms;
	let height = chain.head().unwrap().height;
	assert_eq!(
		(terms.revoke, terms.refund, terms.timeout),
		(height + 1441, height + 1562, height + 1683)
	);
	let message = Packet::offer(&seller.view().unwrap().message().unwrap().encode()?)?;
	buyer.advance(
		&b,
		&fb,
		bm,
		&config,
		Some(Command::Accept {
			expected: message,
			address: address.clone(),
		}),
	)?;
	{
		let view = buyer.view().unwrap();
		assert!(view.armed && !view.ready);
		assert!(
			view.message().is_some(),
			"buyer reply must be visible after accepting"
		);
	}
	let mut messages = 1;
	for _ in 0..2 {
		let reply = {
			buyer
				.view()
				.unwrap()
				.message()
				.map(|p| Command::Import(p.encode().unwrap()))
		};
		if reply.is_some() {
			messages += 1;
		}
		seller.advance(&a, &fa, am, &config, reply)?;
		let reply = {
			seller
				.view()
				.unwrap()
				.message()
				.map(|p| Command::Import(p.encode().unwrap()))
		};
		if reply.is_some() {
			messages += 1;
		}
		buyer.advance(&b, &fb, bm, &config, reply)?;
		seller.session = Some(read(&seller.root.join("session.json"))?);
		buyer.session = Some(read(&buyer.root.join("session.json"))?);
	}
	assert!(seller.view().unwrap().ready && buyer.view().unwrap().ready);
	{
		assert_eq!(messages, 4, "exactly four preparation Slatepacks");
		let mut wrong = match buyer.view().unwrap().outgoing.unwrap() {
			Packet::Round { message } => message,
			_ => unreachable!(),
		};
		wrong.id = Uuid::new_v4();
		assert!(
			buyer
				.advance(
					&b,
					&fb,
					bm,
					&config,
					Some(Command::Import(Packet::Round { message: wrong }.encode()?))
				)
				.is_err()
		);
		assert!(
			buyer
				.advance(
					&b,
					&fb,
					bm,
					&config,
					Some(Command::Import(
						Packet::Release {
							id: Uuid::new_v4(),
							success: "invalid".into()
						}
						.encode()?
					))
				)
				.is_err()
		);
	}
	assert!(seller.reply.is_none());
	assert!(buyer.reply.as_ref().unwrap().payment.is_none());
	assert_eq!(chain.head().unwrap().height, 10);
	seller.advance(&a, &fa, am, &config, Some(Command::Start))?;
	test_framework::award_blocks_to_wallet(&chain, wallet1.clone(), am, 2, false)?;
	buyer.advance(&b, &fb, bm, &config, None)?;
	for (owner, mask, worker) in [(&a, am, &seller), (&b, bm, &buyer)] {
		assert!(
			owner
				.swap(
					mask,
					grin_wallet_api::swap::Request::CancelPreparation {
						record: worker.session.as_ref().unwrap().preparation.record(),
					}
				)
				.is_err()
		);
	}
	let observations = buyer.view().unwrap().chain().unwrap().clone();
	assert!(
		matches!(observations.status.funding, grin_wallet_libwallet::swap::TxState::Confirmed(n) if n >= buyer.view().unwrap().proposal.terms.confirmations)
	);
	assert_eq!(
		observations.status.bitcoin,
		grin_wallet_libwallet::swap::TxState::Absent
	);
	let txs = b.retrieve_txs(bm, true, None, None, None)?.1;
	let deposit = txs
		.iter()
		.find(|tx| tx.swap.as_ref().map(|s| s.kind) == Some(SwapTx::Deposit) && tx.confirmed)
		.unwrap();
	let transfer = txs
		.iter()
		.find(|tx| tx.swap.as_ref().map(|s| s.kind) == Some(SwapTx::Transfer))
		.unwrap();
	let recovery = txs
		.iter()
		.find(|tx| tx.swap.as_ref().map(|s| s.kind) == Some(SwapTx::Recovery))
		.unwrap();
	assert_eq!(deposit.swap.as_ref().map(|s| s.kind), Some(SwapTx::Deposit));
	assert!(deposit.confirmed && visible(deposit));
	assert_eq!(
		transfer.swap.as_ref().map(|s| s.kind),
		Some(SwapTx::Transfer)
	);
	assert!(!transfer.confirmed && visible(transfer));
	assert!(!visible(recovery));
	let info = b.retrieve_summary_info(bm, false, 2)?.1;
	assert_eq!(info.amount_awaiting_finalization, 5_000_000_000);
	assert_eq!(info.total, 0);
	let mut refunded = recovery.clone();
	refunded.confirmed = true;
	assert!(visible(&refunded));

	let payment = buyer.reply.as_ref().unwrap().payment.as_ref().unwrap();
	assert_eq!(observations.address, payment.address);
	assert!(observations.kernels.contains_key("funding"));
	assert!(observations.txid.is_none());
	bitcoin(&[
		"-named",
		"sendtoaddress",
		&format!("address={}", payment.address),
		"amount=0.001",
		"fee_rate=2",
	]);
	bitcoin(&["-generate", "3"]);
	for _ in 0..50 {
		seller.advance(&a, &fa, am, &config, None)?;
		let release = {
			seller.session = Some(read(&seller.root.join("session.json"))?);
			seller
				.view()
				.unwrap()
				.message()
				.filter(|p| matches!(p, Packet::Release { .. }))
				.map(|p| Command::Import(p.encode().unwrap()))
		};
		if release.is_some() {
			let view = seller.view().unwrap();
			let observed = view.chain().unwrap();
			assert!(
				matches!(observed.status.bitcoin, grin_wallet_libwallet::swap::TxState::Confirmed(n) if n >= 2)
			);
			assert!(observed.txid.is_some());
			messages += 1;
		}
		buyer.advance(&b, &fb, bm, &config, release)?;
		if buyer.reply.as_ref().unwrap().action == Action::ClaimGrin {
			break;
		}
		thread::sleep(Duration::from_millis(100));
	}
	assert_eq!(buyer.reply.as_ref().unwrap().action, Action::ClaimGrin);
	{
		assert_eq!(messages, 5, "four preparation messages and one release");
	}
	test_framework::award_blocks_to_wallet(&chain, wallet1.clone(), am, 2, false)?;
	seller.advance(&a, &fa, am, &config, None)?;
	buyer.advance(&b, &fb, bm, &config, None)?;

	let received = b.retrieve_summary_info(bm, true, 2)?.1;
	// The test node also pays the claim block reward to the buyer
	let total = 5_000_000_000 + grin_core::consensus::reward(offer_fee());
	assert_eq!(received.total, total);
	assert_eq!(
		received.amount_currently_spendable,
		total - received.amount_immature
	);
	assert_eq!(received.amount_awaiting_finalization, 0);
	assert_eq!(received.amount_awaiting_confirmation, 0);
	let waiting = b.retrieve_summary_info(bm, false, 10)?.1;
	assert_eq!(waiting.total, total);
	assert_eq!(waiting.amount_currently_spendable, 0);
	assert_eq!(
		waiting.amount_awaiting_confirmation,
		total - waiting.amount_immature
	);
	assert_eq!(waiting.amount_awaiting_finalization, 0);
	let sent = a.retrieve_summary_info(am, true, 2)?.1;
	assert_eq!(sent.amount_awaiting_finalization, 0);

	for (api, mask) in [(&a, am), (&b, bm)] {
		let txs = api.retrieve_txs(mask, true, None, None, None)?.1;
		let transfer = txs
			.iter()
			.find(|tx| tx.swap.as_ref().map(|s| s.kind) == Some(SwapTx::Transfer))
			.unwrap();
		assert_eq!(
			transfer.swap.as_ref().map(|s| s.kind),
			Some(SwapTx::Transfer)
		);
		assert!(transfer.confirmed && visible(transfer));
	}
	assert!(seller.view().unwrap().owns_bitcoin());
	assert!(buyer.view().unwrap().finished());
	assert!(!seller.view().unwrap().paid);
	assert!(seller.reply.as_ref().unwrap().withdrawal.is_some());
	let payout_id = seller.reply.as_ref().unwrap().withdrawal.clone().unwrap();
	let payout = bitcoin(&["getrawtransaction", &payout_id, "true"]);
	let payout_sats = (payout["vout"][0]["value"].as_f64().unwrap() * 100_000_000.0).round() as u64;
	let payout_fee = 100_000 - payout_sats;
	assert_eq!(payout_fee, payout["vsize"].as_u64().unwrap() * 2);
	assert!(payout_fee <= 5000);
	assert!(!seller.view().unwrap().finished());
	assert!(
		seller
			.advance(&a, &fa, am, &config, Some(Command::Archive))
			.is_err()
	);
	// Older versions persisted paid=true immediately after broadcast.
	// On resume this must not bypass confirmation monitoring.
	seller.session.as_mut().unwrap().paid = true;
	seller.store()?;
	seller.session = Some(read(&seller.root.join("session.json"))?);
	seller.reply = None;
	seller.updated = None;
	seller.advance(&a, &fa, am, &config, None)?;
	assert!(!seller.view().unwrap().paid);
	assert_eq!(
		seller.reply.as_ref().unwrap().withdrawal.as_ref(),
		Some(&payout_id)
	);
	bitcoin(&["-generate", "3"]);
	seller.advance(&a, &fa, am, &config, None)?;
	assert!(seller.view().unwrap().paid);
	assert_eq!(
		bitcoin(&["getreceivedbyaddress", &address, "1", "true"]).as_f64(),
		Some((100_000 - payout_fee) as f64 / 100_000_000.0)
	);
	// Confirmation loss must be detected even when the cached view allowed archiving.
	let height = bitcoin(&["getblockcount"]).as_u64().unwrap();
	let block = bitcoin(&["getblockhash", &(height - 2).to_string()]);
	bitcoin(&["invalidateblock", block.as_str().unwrap()]);
	assert!(
		seller
			.advance(&a, &fa, am, &config, Some(Command::Archive))
			.is_err()
	);
	assert!(!seller.view().unwrap().paid);
	assert!(seller.session.is_some());
	bitcoin(&["-generate", "3"]);
	seller.advance(&a, &fa, am, &config, Some(Command::Archive))?;
	assert!(seller.session.is_none());
	running.store(false, Ordering::Relaxed);
	proxy_thread.join().unwrap();
	Ok(())
}

#[test]
fn payment() {
	let id = Uuid::new_v4();
	let mut view = SwapView {
		id,
		role: Role::BuyGrin,
		proposal: Proposal {
			grin: 5_012_500_000,
			bitcoin: 100_000,
			fee: 12_500_000,
			chain: "Testnet".into(),
			network: "testnet4".into(),
			terms: Terms {
				revoke: 100,
				refund: 120,
				timeout: 140,
				confirmations: 2,
				bitcoin_confirmations: 2,
				margin: 12,
			},
		},
		address: String::new(),
		outgoing: None,
		round: 4,
		ready: true,
		armed: true,
		stopped: false,
		paid: false,
		cancelled: false,
		updated: Some(Instant::now()),
		reply: Some(Reply {
			grin: None,
			funding_recovery: false,
			chain: None,
			withdrawal: None,
			payout: None,
			id,
			key: String::new(),
			proof: None,
			action: Action::FundOther,
			funding: None,
			main: None,
			payment: Some(grin_wallet_api::swap::sas::Payment {
				address: "test address".into(),
				amount: 100_000,
				network: "testnet4".into(),
				expires: 96,
			}),
		}),
	};
	assert!(view.payment().is_some());
	view.updated = Some(Instant::now() - Duration::from_secs(11));
	assert!(view.payment().is_none());
	view.updated = Some(Instant::now());
	view.stopped = true;
	assert!(view.payment().is_none());
	view.stopped = false;
	view.reply.as_mut().unwrap().funding = Some("funded".into());
	assert!(view.payment().is_none());
}

#[test]
fn connection_state() {
	let mut status = Connection::default();
	assert!(!status.connected());
	status.checked = Some(Instant::now());
	status.height = Some(10);
	assert!(status.connected());
	status.checked = Some(Instant::now() - Duration::from_secs(46));
	assert!(!status.connected());
	status.checked = Some(Instant::now());
	status.error = Some("offline".into());
	assert!(!status.connected());
}

#[test]
fn commands() {
	let (tx, rx) = mpsc::channel();
	let service = Service {
		account: "default".into(),
		tx,
		snapshot: Arc::new(RwLock::new(Snapshot {
			timing: Timing::default(),
			connection: Connection::default(),
			bitcoin: default_bitcoin_config(),
			view: None,
			busy: false,
			error: None,
		})),
		active: Arc::new(AtomicBool::new(false)),
		alive: Arc::new(AtomicBool::new(true)),
	};
	let receipt = service.send(Command::Import("invalid".into())).unwrap();
	assert!(service.send(Command::Retry).is_err());
	assert!(receipt.result().is_none());
	let (_, worker_receipt) = rx.recv().unwrap();
	worker_receipt.complete(Err("Invalid message".into()));
	{
		let mut snapshot = service.snapshot.write();
		snapshot.busy = false;
		snapshot.error = None;
	}
	let retry = service.send(Command::Retry).unwrap();
	let (_, worker_receipt) = rx.recv().unwrap();
	worker_receipt.complete(Ok(()));
	assert_eq!(retry.result(), Some(Ok(())));
	assert_eq!(receipt.result(), Some(Err("Invalid message".into())));
	service.snapshot.write().busy = false;
	drop(rx);
	assert!(service.send(Command::Retry).is_err());
	assert!(!service.snapshot().busy);
	service.active.store(true, Ordering::SeqCst);
	service.load_failed(&fail("broken session"));
	assert!(!service.active());
	assert!(!service.alive());
	assert!(service.snapshot().error.unwrap().contains("broken session"));
}

#[test]
fn proxy_settings() {
	let original = default_bitcoin_config();
	let mut json = serde_json::to_value(&original).unwrap();
	json.as_object_mut().unwrap().remove("proxy");
	assert_eq!(
		serde_json::from_value::<BitcoinConfig>(json).unwrap(),
		original
	);
	let mut configured = original;
	configured.proxy = Some("socks5h://127.0.0.1:9050".into());
	let encoded = serde_json::to_vec(&configured).unwrap();
	assert_eq!(
		serde_json::from_slice::<BitcoinConfig>(&encoded).unwrap(),
		configured
	);
}

#[test]
fn load_error() {
	let root = std::env::temp_dir().join(format!("grim-load-{}", Uuid::new_v4()));
	private_dir(&root).unwrap();
	assert!(
		read_optional::<Session>(&root.join("session.json"))
			.unwrap()
			.is_none()
	);
	fs::write(root.join("session.json"), b"broken").unwrap();
	assert!(Worker::load(root.clone()).is_err());
	assert_eq!(fs::read(root.join("session.json")).unwrap(), b"broken");
	fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_path() {
	for account in ["default", "Wallet A", "Grüße"] {
		let previous: String = account
			.as_bytes()
			.iter()
			.map(|b| format!("{b:02x}"))
			.collect();
		assert_eq!(account.as_bytes().to_hex(), previous);
	}
}

#[test]
fn history() -> Result<(), Error> {
	use grin_keychain::{ExtKeychain, Keychain};
	use grin_wallet_impls::HTTPNodeClient;
	use grin_wallet_libwallet::{SlateState, TxLogEntry, TxLogEntryType, WalletBackend};
	let root = std::env::temp_dir().join(format!("grim-history-{}", Uuid::new_v4()));
	let records = root.join("swaps").join("default".as_bytes().to_hex());
	private_dir(&records)?;
	let id = Uuid::new_v4();
	let refund = Uuid::new_v4();
	let record = serde_json::json!({"preparation": {
		"role": "SellGrin",
		"slates": {
			"fund": serde_json::json!({"id": id}).to_string(),
			"refund": serde_json::json!({"id": refund}).to_string(),
			"outputs": "[]"
		}
	}});
	save(&records.join(format!("{}.json", Uuid::new_v4())), &record)?;
	fs::write(records.join("bitcoin.json"), "unrelated settings").unwrap();
	let client = HTTPNodeClient::new("http://127.0.0.1:1", None, Duration::from_secs(1))?;
	let mut wallet = WalletBackend::<_, ExtKeychain>::new(root.to_str().unwrap(), client)?;
	wallet.set_keychain(ExtKeychain::from_seed(&[42; 32], true)?, false, true)?;
	let parent = wallet.parent_key_id();
	let mut tx = TxLogEntry::new(parent.clone(), TxLogEntryType::TxReceived, 0);
	tx.tx_slate_id = Some(refund);
	tx.tx_slate_state = Some(SlateState::Atomic2);
	{
		let mut batch = wallet.batch(None)?;
		batch.save_tx_log_entry(tx, &parent)?;
		batch.commit()?;
	}
	for _ in 0..2 {
		import_records(&mut wallet, None, &root.join("swaps"))?;
		assert_eq!(wallet.parent_key_id(), parent);
	}
	let mut tx = wallet.tx_log_iter()?.next().unwrap()?;
	assert_eq!(tx.swap.as_ref().unwrap().kind, SwapTx::Recovery);
	assert!(!visible(&tx));
	assert!(grin_wallet_libwallet::swap::records::check_edit(&tx).is_err());
	tx.confirmed = true;
	assert!(visible(&tx));
	tx.tx_type = TxLogEntryType::TxReceivedCancelled;
	assert!(grin_wallet_libwallet::swap::records::check_edit(&tx).is_err());
	drop(wallet);
	fs::remove_dir_all(root).unwrap();
	Ok(())
}

#[test]
fn swap_record_actions() {
	use crate::wallet::types::WalletTx;
	use grin_wallet_libwallet::{TxLogEntry, TxLogEntryType};
	let tx = TxLogEntry::new(grin_keychain::Identifier::zero(), TxLogEntryType::TxSent, 0);
	let mut ordinary = WalletTx::new(tx, None, None, None, None, None);
	assert!(ordinary.can_cancel());
	assert!(!ordinary.can_delete());
	ordinary.data.confirmed = true;
	assert!(ordinary.can_delete());
	for kind in [
		SwapTx::Deposit,
		SwapTx::Transfer,
		SwapTx::Recovery,
		SwapTx::Internal,
	] {
		let mut swap = ordinary.clone();
		swap.swap = Some(kind);
		assert!(!swap.can_delete());
		swap.data.confirmed = false;
		assert!(!swap.can_cancel());
		swap.data.tx_type = TxLogEntryType::TxReceivedCancelled;
		assert!(!swap.can_delete());
	}
}
