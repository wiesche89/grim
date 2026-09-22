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

//! Persistent swap worker, independent of the selected GUI tab

use super::Wallet;
use grin_core::libtx::tx_fee;
use grin_util::ToHex;
use grin_wallet_api::swap::{
	Reply,
	negotiation::{Message, Preparation, Proposal},
	pack::Packet,
};
use grin_wallet_config::types::BitcoinConfig;
use grin_wallet_libwallet::{
	Error,
	swap::{Action, Role},
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
		mpsc::{self, Sender},
	},
	thread,
	time::{Duration, Instant},
};
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_secs(3);
const PROBE_INTERVAL: Duration = Duration::from_secs(30);
const CONNECTION_TTL: Duration = Duration::from_secs(45);
const CHAIN_TTL: Duration = Duration::from_secs(10);
const CLOSED: &str = "Open the wallet again to resume swaps";

mod bitcoin;
pub use bitcoin::{NETWORKS, explorer_url, server, validate_address};
use bitcoin::{default_bitcoin_config, node};

pub fn offer_fee() -> u64 {
	tx_fee(1, 1, 1)
}

pub fn validate_amounts(grin: u64, bitcoin: u64) -> bool {
	grin.checked_add(offer_fee())
		.is_some_and(|total| Proposal::validate_amounts(total, bitcoin, offer_fee()).is_ok())
}

#[derive(Clone, Default)]
pub struct Connection {
	pub checked: Option<Instant>,
	pub height: Option<u64>,
	pub error: Option<String>,
}
impl Connection {
	pub fn connected(&self) -> bool {
		self.height.is_some()
			&& self.error.is_none()
			&& self.checked.is_some_and(|t| t.elapsed() < CONNECTION_TTL)
	}
	fn probe(config: &BitcoinConfig) -> Self {
		match node(config).and_then(|n| n.tip()) {
			Ok((height, _)) => Self {
				checked: Some(Instant::now()),
				height: Some(height),
				error: None,
			},
			Err(error) => Self {
				checked: Some(Instant::now()),
				height: None,
				error: Some(error.to_string()),
			},
		}
	}
}

pub use grin_wallet_libwallet::swap::timing::Timing;

mod history;
pub use grin_wallet_libwallet::swap::records::TxKind as SwapTx;
pub(super) use history::{import_records, visible};

mod storage;
use storage::fail;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
	preparation: Preparation,
	#[serde(default)]
	release: Option<String>,
	folder: Option<PathBuf>,
	armed: bool,
	stopped: bool,
	paid: bool,
	#[serde(default)]
	cancelled: bool,
}

#[derive(Clone)]
pub struct SwapView {
	pub id: Uuid,
	pub role: Role,
	pub proposal: Proposal,
	pub address: String,
	pub outgoing: Option<Packet>,
	pub round: u8,
	pub ready: bool,
	pub armed: bool,
	pub stopped: bool,
	pub paid: bool,
	pub cancelled: bool,
	pub reply: Option<Reply>,
	pub updated: Option<Instant>,
}

fn owns_bitcoin(role: Role, reply: Option<&Reply>) -> bool {
	reply.is_some_and(|r| {
		r.funding.is_some()
			&& matches!(
				(role, r.action),
				(Role::SellGrin, Action::Complete) | (Role::BuyGrin, Action::Refunded)
			)
	})
}

fn finished(role: Role, reply: Option<&Reply>, cancelled: bool, paid: bool) -> bool {
	cancelled
		|| (reply.is_some_and(|r| {
			matches!(
				r.action,
				Action::Complete | Action::Refunded | Action::TimedOut
			)
		}) && (!owns_bitcoin(role, reply) || paid))
}

impl SwapView {
	fn fresh(&self) -> bool {
		self.updated.is_some_and(|at| at.elapsed() < CHAIN_TTL)
	}

	/// Last observation for display only, including stale observations
	pub fn last_chain(&self) -> Option<&grin_wallet_api::swap::sas::Chain> {
		self.reply.as_ref()?.chain.as_ref()
	}

	pub fn chain(&self) -> Option<&grin_wallet_api::swap::sas::Chain> {
		if !self.fresh() {
			return None;
		}
		self.last_chain()
	}

	/// Message available for the next manual exchange
	pub fn message(&self) -> Option<&Packet> {
		if self.cancelled || self.stopped || self.finished() || self.owns_bitcoin() {
			return None;
		}
		self.outgoing.as_ref().filter(|p| match p {
			Packet::Round { message } => {
				message.version == 2
					&& (!self.ready || message.round == 3)
					&& !self
						.reply
						.as_ref()
						.and_then(|r| r.chain.as_ref())
						.is_some_and(|c| {
							matches!(
								c.status.funding,
								grin_wallet_libwallet::swap::TxState::Confirmed(_)
							)
						})
			}
			Packet::Release { .. } => !self.stopped && !self.finished(),
		})
	}

	pub fn payment(&self) -> Option<&grin_wallet_api::swap::sas::Payment> {
		if !self.ready || !self.armed || self.stopped || self.role != Role::BuyGrin || !self.fresh()
		{
			return None;
		}
		let reply = self.reply.as_ref()?;
		if reply.action != Action::FundOther || reply.funding.is_some() {
			return None;
		}
		reply.payment.as_ref()
	}

	pub fn owns_bitcoin(&self) -> bool {
		owns_bitcoin(self.role, self.reply.as_ref())
	}
	pub fn finished(&self) -> bool {
		finished(self.role, self.reply.as_ref(), self.cancelled, self.paid)
	}
}

#[derive(Clone)]
pub struct Snapshot {
	pub timing: Timing,
	pub connection: Connection,
	pub bitcoin: BitcoinConfig,
	pub view: Option<Arc<SwapView>>,
	pub busy: bool,
	pub error: Option<String>,
}

impl Snapshot {
	fn apply(
		&mut self,
		worker: &mut Worker,
		result: Result<Option<BitcoinConfig>, Error>,
		requested: bool,
	) {
		match result {
			Ok(Some(config)) => {
				self.bitcoin = config;
				self.connection = Connection::default();
				self.error = None;
			}
			Ok(None) => {
				if requested || worker.updated.is_some() {
					self.error = None;
				}
			}
			Err(e) => {
				self.error = Some(e.to_string());
				worker.updated = None;
			}
		}
		self.timing = worker.timing;
		self.view = worker.view().map(Arc::new);
		if requested {
			self.busy = false;
		}
	}
}

pub enum Command {
	Configure(BitcoinConfig),
	SetTiming(Timing),
	Create {
		grin: u64,
		bitcoin: u64,
		address: String,
	},
	Accept {
		expected: Message,
		address: String,
	},
	Import(String),
	Start,
	Abort,
	Archive,
	Retry,
}

#[derive(Clone, Default)]
pub struct Receipt(Arc<RwLock<Option<Result<(), String>>>>);

impl Receipt {
	pub fn result(&self) -> Option<Result<(), String>> {
		self.0.read().clone()
	}
	fn complete(&self, result: Result<(), String>) {
		*self.0.write() = Some(result);
	}
}

#[derive(Clone)]
pub struct Service {
	pub(crate) account: String,
	tx: Sender<(Command, Receipt)>,
	snapshot: Arc<RwLock<Snapshot>>,
	active: Arc<AtomicBool>,
	alive: Arc<AtomicBool>,
}

impl Service {
	pub fn snapshot(&self) -> Snapshot {
		self.snapshot.read().clone()
	}
	pub fn active(&self) -> bool {
		self.active.load(Ordering::SeqCst)
	}
	pub fn alive(&self) -> bool {
		self.alive.load(Ordering::SeqCst)
	}
	pub fn send(&self, command: Command) -> Result<Receipt, Error> {
		let mut snapshot = self.snapshot.write();
		if !self.alive() {
			return Err(fail(CLOSED));
		}
		if snapshot.busy {
			return Err(fail("Wait for the current action to finish"));
		}
		let receipt = Receipt::default();
		let active = matches!(command, Command::Create { .. } | Command::Accept { .. });
		self.tx
			.send((command, receipt.clone()))
			.map_err(|_| fail(CLOSED))?;
		if active {
			self.active.store(true, Ordering::SeqCst);
		}
		snapshot.busy = true;
		Ok(receipt)
	}
	pub(crate) fn start(wallet: Wallet) -> Self {
		let config = wallet.get_config();
		let account = config.account.as_bytes().to_hex();
		let base = config.get_data_path().join("swaps");
		let root = base.join(account);
		let default = default_bitcoin_config();
		let (tx, rx) = mpsc::channel();
		let service = Self {
			account: config.account.clone(),
			tx,
			snapshot: Arc::new(RwLock::new(Snapshot {
				timing: Timing::default(),
				bitcoin: default,
				connection: Connection::default(),
				view: None,
				busy: true,
				error: None,
			})),
			active: Arc::new(AtomicBool::new(true)),
			alive: Arc::new(AtomicBool::new(true)),
		};
		let shared = service.clone();
		thread::spawn(move || {
			grin_core::global::set_local_chain_type(config.chain_type);
			let (mut worker, bitcoin) = match Worker::load(root) {
				Ok(loaded) => loaded,
				Err(error) => {
					let mut snapshot = shared.snapshot.write();
					snapshot.error = Some(error.to_string());
					snapshot.busy = false;
					shared.alive.store(false, Ordering::SeqCst);
					return;
				}
			};
			shared.snapshot.write().bitcoin = bitcoin;
			{
				let mut snapshot = shared.snapshot.write();
				snapshot.timing = worker.timing;
				snapshot.view = worker.view().map(Arc::new);
				snapshot.busy = false;
			}
			while wallet.is_open()
				&& !wallet.is_closing()
				&& wallet.get_config().account == config.account
			{
				let request = match rx.recv_timeout(POLL_INTERVAL) {
					Ok(c) => Some(c),
					Err(mpsc::RecvTimeoutError::Timeout) => None,
					Err(_) => break,
				};
				let guard = wallet.swap_lock();
				if !wallet.is_open()
					|| wallet.is_closing()
					|| wallet.get_config().account != config.account
				{
					if let Some((_, receipt)) = request {
						receipt.complete(Err(CLOSED.into()));
					}
					break;
				}
				let (command, receipt) = request.unzip();
				let bitcoin = shared.snapshot.read().bitcoin.clone();
				let requested = command.is_some();
				let reconnect =
					matches!(command, Some(Command::Configure(_)) | Some(Command::Retry));
				let started = Instant::now();
				let result = worker.run(&wallet, &bitcoin, command);
				log::debug!(
					"Swap update: {} ms, success: {}",
					started.elapsed().as_millis(),
					result.is_ok()
				);
				let outcome = result.as_ref().map(|_| ()).map_err(ToString::to_string);
				let mut snapshot = shared.snapshot.write();
				snapshot.apply(&mut worker, result, requested);
				shared.active.store(
					snapshot.view.as_ref().is_some_and(|v| !v.finished()),
					Ordering::SeqCst,
				);
				let check = reconnect
					|| snapshot
						.connection
						.checked
						.is_none_or(|at| at.elapsed() >= PROBE_INTERVAL);
				let bitcoin = snapshot.bitcoin.clone();
				drop(snapshot);
				if let Some(receipt) = receipt {
					receipt.complete(outcome);
				}
				drop(guard);
				if check {
					let connection = Connection::probe(&bitcoin);
					shared.snapshot.write().connection = connection;
				}
			}
			{
				let mut snapshot = shared.snapshot.write();
				shared.alive.store(false, Ordering::SeqCst);
				snapshot.busy = false;
			}
			for (_, receipt) in rx.try_iter() {
				receipt.complete(Err(CLOSED.into()));
			}
		});
		service
	}
}

mod worker;
use worker::Worker;

#[cfg(test)]
mod tests;
