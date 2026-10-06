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

use super::storage::{fail, private_dir, read_optional, save};
use super::{
	Command, Session, SwapView, Timing, Wallet, default_bitcoin_config, finished, node, offer_fee,
	owns_bitcoin,
};
use grin_wallet_api::{
	Foreign, Owner,
	swap::{
		Reply,
		negotiation::{Driver, Preparation, Proposal},
		pack::Packet,
		sas::Request,
	},
};
use grin_wallet_config::types::BitcoinConfig;
use grin_wallet_libwallet::{Error, swap::Role};
use std::{fs, path::PathBuf, time::Instant};

pub(super) struct Worker {
	pub(super) timing: Timing,
	pub(super) root: PathBuf,
	pub(super) session: Option<Session>,
	pub(super) reply: Option<Reply>,
	pub(super) updated: Option<Instant>,
}

impl Worker {
	pub(super) fn load(root: PathBuf) -> Result<(Self, BitcoinConfig), Error> {
		private_dir(
			root.parent()
				.ok_or_else(|| fail("Missing swap directory"))?,
		)?;
		private_dir(&root)?;
		let bitcoin =
			read_optional(&root.join("bitcoin.json"))?.unwrap_or_else(default_bitcoin_config);
		let timing = read_optional(&root.join("timing.json"))?.unwrap_or_default();
		let session = read_optional(&root.join("session.json"))?;
		Ok((
			Self {
				root,
				timing,
				session,
				reply: None,
				updated: None,
			},
			bitcoin,
		))
	}

	pub(super) fn view(&self) -> Option<SwapView> {
		self.session.as_ref().map(|s| SwapView {
			id: s.preparation.id,
			role: s.preparation.role,
			proposal: s.preparation.proposal.clone(),
			address: s.preparation.address.clone(),
			outgoing: if s.cancelled || s.stopped {
				None
			} else {
				s.release
					.as_ref()
					.filter(|_| s.preparation.role == Role::SellGrin)
					.map(|success| Packet::Release {
						id: s.preparation.id,
						success: success.clone(),
					})
					.or_else(|| {
						s.preparation
							.outgoing
							.clone()
							.map(|message| Packet::Round { message })
					})
			},
			round: s.preparation.next_round(),
			ready: s.preparation.ready,
			armed: s.armed,
			stopped: s.stopped,
			paid: s.paid,
			cancelled: s.cancelled,
			reply: self.reply.clone(),
			updated: self.updated,
		})
	}
	pub(super) fn store(&self) -> Result<(), Error> {
		save(
			&self.root.join("session.json"),
			self.session.as_ref().ok_or_else(|| fail("No swap"))?,
		)
	}
	pub(super) fn run(
		&mut self,
		wallet: &Wallet,
		bitcoin: &BitcoinConfig,
		command: Option<Command>,
	) -> Result<Option<BitcoinConfig>, Error> {
		if !wallet.swaps_enabled() {
			return Err(fail("Swaps are enabled on Grin testnet"));
		}
		if let Some(Command::SetTiming(timing)) = command.as_ref() {
			if self.session.is_some() {
				return Err(fail(
					"Finish the current swap before changing its deadlines",
				));
			}
			timing.validate()?;
			save(&self.root.join("timing.json"), timing)?;
			self.timing = *timing;
			return Ok(None);
		}
		if let Some(Command::Configure(ref config)) = command {
			if self.session.is_some() {
				return Err(fail(
					"Finish the current swap before changing the Bitcoin connection",
				));
			}
			node(config)?;
			save(&self.root.join("bitcoin.json"), config)?;
			return Ok(Some(config.clone()));
		}
		if self.session.is_none() && command.is_none() {
			return Ok(None);
		}
		super::bitcoin::test_network(&bitcoin.network)?;
		let path = wallet.get_config().get_wallet_path();
		let mask = wallet.keychain_mask();
		let instance = wallet.swap_instance()?;
		let owner =
			Owner::new(instance.clone(), None, path.clone()).with_bitcoin_config(bitcoin.clone());
		let foreign = Foreign::new(instance, path, mask.clone(), None, false);
		self.advance(&owner, &foreign, mask.as_ref(), bitcoin, command)?;
		Ok(None)
	}

	fn receive<L, C, K>(
		&mut self,
		api: &Driver,
		owner: &Owner<L, C, K>,
		mask: Option<&grin_util::secp::SecretKey>,
		packet: Packet,
	) -> Result<(), Error>
	where
		L: grin_wallet_libwallet::WalletLCProvider<'static, C, K> + 'static,
		C: grin_wallet_libwallet::NodeClient + 'static,
		K: grin_keychain::Keychain + 'static,
	{
		let session = self.session.as_mut().ok_or_else(|| fail("No swap"))?;
		if session.cancelled {
			return Err(fail("Swap cancelled"));
		}
		match packet {
			Packet::Round { message } => {
				if session.stopped {
					return Err(fail("Swap stopped"));
				}
				let mut saved = session.clone();
				let path = self.root.join("session.json");
				session
					.preparation
					.receive(api, message, &mut |preparation| {
						saved.preparation = preparation.clone();
						save(&path, &saved)
					})?;
			}
			Packet::Release { id, success } => {
				if id != session.preparation.id || session.preparation.role != Role::BuyGrin {
					return Err(fail("Unexpected swap release"));
				}
				let swap = session
					.preparation
					.swap
					.ok_or_else(|| fail("Swap is not ready"))?;
				owner.sas(
					mask,
					Request::Receive {
						id: swap,
						funding: None,
						success: Some(success.clone()),
					},
				)?;
				session.release = Some(success);
			}
		}
		self.store()
	}

	fn handle_command<L, C, K>(
		&mut self,
		owner: &Owner<L, C, K>,
		api: &Driver,
		mask: Option<&grin_util::secp::SecretKey>,
		bitcoin: &BitcoinConfig,
		command: Option<Command>,
	) -> Result<bool, Error>
	where
		L: grin_wallet_libwallet::WalletLCProvider<'static, C, K> + 'static,
		C: grin_wallet_libwallet::NodeClient + 'static,
		K: grin_keychain::Keychain + 'static,
	{
		match command {
			Some(Command::Create {
				grin,
				bitcoin: amount,
				address,
			}) => {
				if self.session.is_some() {
					return Err(fail("Finish the current swap first"));
				}
				node(bitcoin)?;
				let tip = owner.node_height(mask)?;
				if !tip.updated_from_node {
					return Err(fail("Grin node is unavailable"));
				}
				let proposal = Proposal {
					grin,
					bitcoin: amount,
					fee: offer_fee(),
					chain: format!("{:?}", grin_core::global::get_chain_type()),
					network: bitcoin.network.clone(),
					terms: self.timing.terms(tip.height)?,
				};
				self.session = Some(Session {
					preparation: Preparation::new(proposal, address)?,
					release: None,
					folder: None,
					armed: false,
					stopped: false,
					paid: false,
					cancelled: false,
				});
				self.store()?;
			}
			Some(Command::Accept { expected, address }) => {
				if self.session.is_some() {
					return Err(fail("Finish the current swap first"));
				}
				let offer = expected;
				if offer.proposal.network != bitcoin.network {
					return Err(fail("Offer or network changed"));
				}
				node(bitcoin)?;
				let tip = owner.node_height(mask)?;
				if !tip.updated_from_node || !offer.proposal.terms.open(tip.height) {
					return Err(fail("Offer expired or node unavailable"));
				}
				self.timing.accepts(offer.proposal.terms, tip.height)?;
				self.session = Some(Session {
					preparation: Preparation::accept(&offer, address)?,
					release: None,
					folder: None,
					armed: true,
					stopped: false,
					paid: false,
					cancelled: false,
				});
				self.store()?;
				self.receive(api, owner, mask, Packet::Round { message: offer })?;
			}
			Some(Command::Import(text)) => {
				self.receive(api, owner, mask, Packet::decode(&text)?)?;
			}
			Some(Command::Withdraw { fee }) => {
				let session = self.session.as_mut().ok_or_else(|| fail("No swap"))?;
				let id = session
					.preparation
					.swap
					.ok_or_else(|| fail("Swap is not ready"))?;
				session.paid = false;
				let reply = owner.sas(mask, Request::Withdraw { id, fee })?;
				self.reply = Some(reply);
				self.store()?;
			}
			Some(Command::Archive) => {
				if let Some(session) = self
					.session
					.as_ref()
					.filter(|s| s.preparation.ready && s.armed && !s.cancelled)
				{
					let id = session
						.preparation
						.swap
						.ok_or_else(|| fail("Missing swap id"))?;
					self.reply = Some(owner.sas(mask, Request::Step { id })?);
					self.updated = Some(Instant::now());
				}
				// A saved broadcast flag is not proof of confirmation after restart or reorg.
				if let Some(session) = self
					.session
					.as_mut()
					.filter(|s| owns_bitcoin(s.preparation.role, self.reply.as_ref()))
				{
					session.paid = false;
					let id = session
						.preparation
						.swap
						.ok_or_else(|| fail("Missing swap id"))?;
					let reply = owner.sas(mask, Request::WithdrawAuto { id })?;
					session.paid = payout_confirmed(
						&reply,
						session.preparation.proposal.terms.bitcoin_confirmations,
					);
					self.reply = Some(reply);
					self.store()?;
				}
				let Some(s) = self.session.as_ref().filter(|s| {
					finished(s.preparation.role, self.reply.as_ref(), s.cancelled, s.paid)
				}) else {
					return Err(fail("Swap is not settled"));
				};
				save(&self.root.join(format!("{}.json", s.preparation.id)), s)?;
				fs::remove_file(self.root.join("session.json")).map_err(fail)?;
				self.session = None;
				self.reply = None;
				self.updated = None;
				return Ok(true);
			}
			Some(Command::Start) => {
				let s = self.session.as_mut().ok_or_else(|| fail("No swap"))?;
				if !s.preparation.ready || s.stopped {
					return Err(fail("Swap is not ready"));
				}
				s.armed = true;
				self.store()?;
			}
			Some(Command::Abort) => {
				let s = self.session.as_mut().ok_or_else(|| fail("No swap"))?;
				s.stopped = true;
				self.store()?;
			}

			_ => (),
		}
		Ok(false)
	}

	pub(super) fn advance<L, C, K>(
		&mut self,
		owner: &Owner<L, C, K>,
		foreign: &(dyn grin_wallet_api::ForeignRpc + 'static),
		mask: Option<&grin_util::secp::SecretKey>,
		bitcoin: &BitcoinConfig,
		command: Option<Command>,
	) -> Result<(), Error>
	where
		L: grin_wallet_libwallet::WalletLCProvider<'static, C, K> + 'static,
		C: grin_wallet_libwallet::NodeClient + 'static,
		K: grin_keychain::Keychain + 'static,
	{
		if self.session.as_ref().is_some_and(|s| s.folder.is_some()) {
			return Err(fail("This saved swap uses the retired folder transport"));
		}
		let api = Driver {
			owner,
			foreign,
			mask,
		};
		if self.handle_command(owner, &api, mask, bitcoin, command)? {
			return Ok(());
		}
		let Some(session) = self.session.as_mut() else {
			return Ok(());
		};
		if session.cancelled {
			return Ok(());
		}
		if session.stopped {
			if let Some(id) = session.preparation.swap {
				owner.sas(mask, Request::Abort { id })?;
			}
			if !session.preparation.ready
				|| (session.preparation.role == Role::SellGrin && !session.armed)
			{
				session.preparation.cancel(&api)?;
				session.cancelled = true;
				self.store()?;
				return Ok(());
			}
		}
		let mut saved = session.clone();
		let state_path = self.root.join("session.json");
		let mut checkpoint = |preparation: &Preparation| {
			saved.preparation = preparation.clone();
			save(&state_path, &saved)
		};
		if session.preparation.role == Role::SellGrin
			&& session.preparation.outgoing.is_none()
			&& !session.preparation.ready
		{
			session.preparation.start(&api, &mut checkpoint)?;
		}
		if session.preparation.ready && session.armed {
			let swap = session
				.preparation
				.swap
				.ok_or_else(|| fail("Missing swap id"))?;
			let reply = owner.sas(mask, Request::Step { id: swap })?;
			if let Some(success) = &reply.main
				&& session.preparation.role == Role::SellGrin
			{
				session.release = Some(success.clone());
			}
			save(&state_path, session)?;
			self.reply = Some(reply);
			self.updated = Some(Instant::now());
		}
		if let Some(session) = self
			.session
			.as_mut()
			.filter(|s| owns_bitcoin(s.preparation.role, self.reply.as_ref()))
		{
			session.paid = false;
			let mut reply = owner.sas(
				mask,
				Request::WithdrawAuto {
					id: session
						.preparation
						.swap
						.ok_or_else(|| fail("Missing swap id"))?,
				},
			)?;
			reply.chain = self.reply.as_ref().and_then(|r| r.chain.clone());
			reply.grin = self.reply.as_ref().and_then(|r| r.grin);
			session.paid = payout_confirmed(
				&reply,
				session.preparation.proposal.terms.bitcoin_confirmations,
			);
			self.reply = Some(reply);
			self.store()?;
		}

		Ok(())
	}
}

fn payout_confirmed(reply: &Reply, confirmations: u64) -> bool {
	reply
		.payout
		.as_ref()
		.is_some_and(|p| p.status.confirmed(confirmations))
}

#[cfg(test)]
mod tests {
	use super::*;
	use grin_wallet_api::swap::sas::Payout;
	use grin_wallet_libwallet::swap::{Action, TxState};
	#[test]
	fn payout_requires_fresh_confirmations() {
		let mut reply = Reply {
			grin: None,
			funding_recovery: false,
			id: uuid::Uuid::new_v4(),
			key: String::new(),
			action: Action::Complete,
			chain: None,
			withdrawal: Some("saved transaction".into()),
			payout: None,
			payment: None,
			proof: None,
			funding: Some("funding".into()),
			main: None,
		};
		assert!(!payout_confirmed(&reply, 2));
		for (status, confirmed) in [
			(TxState::Absent, false),
			(TxState::Pending, false),
			(TxState::Confirmed(1), false),
			(TxState::Confirmed(2), true),
			(TxState::Conflicted, false),
		] {
			reply.payout = Some(Payout {
				status,
				fee: 222,
				max_fee: 5000,
				can_replace: true,
			});
			assert_eq!(payout_confirmed(&reply, 2), confirmed);
			assert!(!payout_confirmed(&reply, 0));
		}
	}
}
