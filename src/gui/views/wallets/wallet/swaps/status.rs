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

use super::*;

pub(super) struct Status {
	pub phase: Option<u8>,
	pub title: &'static str,
	pub hint: &'static str,
}

impl Status {
	pub fn new(view: &SwapView, copied: Option<&Packet>, importing: bool) -> Self {
		let phase = phase(view);
		let (title, hint) = guide(view, copied, importing, phase);
		Self { phase, title, hint }
	}
}

pub(super) fn role_hint(role: Role, seller: &'static str, buyer: &'static str) -> &'static str {
	match role {
		Role::SellGrin => seller,
		Role::BuyGrin => buyer,
	}
}

pub(super) fn next(view: &SwapView) -> Option<&'static str> {
	if !view.ready
		|| !view.armed
		|| view.cancelled
		|| view.stopped
		|| view.finished()
		|| view.owns_bitcoin()
	{
		return None;
	}
	let Some(chain) = view.chain() else {
		return Some("swaps.chain_wait");
	};
	let status = chain.status;
	let terms = view.proposal.terms;
	let confirmed = |s, required| matches!(s, TxState::Confirmed(n) if n >= required);
	if status.revoke != TxState::Absent
		|| status.refund != TxState::Absent
		|| status.timeout != TxState::Absent
	{
		return Some(role_hint(
			view.role,
			"swaps.recovery_seller",
			"swaps.recovery_buyer",
		));
	}
	if status.success != TxState::Absent
		|| view
			.reply
			.as_ref()
			.is_some_and(|r| r.action == Action::ClaimGrin)
	{
		return Some(role_hint(
			view.role,
			"swaps.wait_receipt_seller",
			"swaps.wait_receipt_buyer",
		));
	}
	if !confirmed(status.funding, terms.confirmations) {
		return Some(if view.role == Role::SellGrin {
			"swaps.wait_grin_seller"
		} else {
			"swaps.wait_grin_buyer"
		});
	}
	if status.bitcoin == TxState::Conflicted
		|| (status.bitcoin_started && status.bitcoin == TxState::Absent)
	{
		return Some(role_hint(
			view.role,
			"swaps.check_payment_seller",
			"swaps.check_payment_buyer",
		));
	}
	if status.bitcoin == TxState::Absent {
		if view.role == Role::BuyGrin {
			return Some(if view.payment().is_some() {
				"swaps.pay_hint"
			} else {
				"swaps.chain_wait"
			});
		}
		return Some(if terms.open(status.height) {
			"swaps.wait_buyer"
		} else {
			role_hint(view.role, "swaps.recovery_seller", "swaps.recovery_buyer")
		});
	}
	if !confirmed(status.bitcoin, terms.bitcoin_confirmations) {
		return Some(role_hint(
			view.role,
			"swaps.wait_bitcoin_seller",
			"swaps.wait_bitcoin_buyer",
		));
	}
	Some(if view.role == Role::BuyGrin {
		"swaps.need_release"
	} else {
		"swaps.wait_claim"
	})
}

pub(super) fn phase(view: &SwapView) -> Option<u8> {
	let action = view.reply.as_ref().map(|r| r.action);
	if view.cancelled
		|| view.reply.as_ref().and_then(|r| r.grin).is_some_and(|g| {
			g.revoke != TxState::Absent
				|| g.refund != TxState::Absent
				|| g.timeout != TxState::Absent
		}) || view.stopped
		|| matches!(
			action,
			Some(
				Action::RevokeGrin
					| Action::RefundGrin
					| Action::RefundOther
					| Action::Refunded
					| Action::TimeoutGrin
					| Action::TimedOut
			)
		) {
		None
	} else if view.finished() {
		Some(3)
	} else if view.chain().is_some_and(|c| {
		c.status
			.bitcoin
			.confirmed(view.proposal.terms.bitcoin_confirmations)
	}) || matches!(view.outgoing, Some(Packet::Release { .. }))
		|| matches!(
			action,
			Some(
				Action::Release
					| Action::ClaimGrin
					| Action::ClaimOther
					| Action::OwnBitcoin
					| Action::Complete
			)
		) {
		Some(2)
	} else if view.ready {
		Some(1)
	} else {
		Some(0)
	}
}

fn guide(
	view: &SwapView,
	copied: Option<&Packet>,
	importing: bool,
	phase: Option<u8>,
) -> (&'static str, &'static str) {
	let action = view.reply.as_ref().map(|r| r.action);
	if view.cancelled {
		("swaps.cancelled", "swaps.cancel_hint")
	} else if action == Some(Action::TimedOut) {
		("swaps.timeout_title", "swaps.timeout_hint")
	} else if view.reply.as_ref().is_some_and(|r| r.funding_recovery) {
		(
			"swaps.funding_recovery_title",
			"swaps.funding_recovery_hint",
		)
	} else if view.owns_bitcoin() && !view.paid {
		("swaps.withdraw_title", "swaps.withdraw_hint")
	} else if view.finished() {
		if view.paid {
			if action == Some(Action::Refunded) {
				("swaps.bitcoin_refund_title", "swaps.bitcoin_refund_hint")
			} else {
				("swaps.paid_title", "swaps.paid_hint")
			}
		} else if action == Some(Action::Refunded) {
			(
				"swaps.refunded",
				if view.role == Role::SellGrin {
					"swaps.refund_hint"
				} else {
					"swaps.refund_buy_hint"
				},
			)
		} else {
			("swaps.complete", "swaps.complete_hint")
		}
	} else if phase.is_none() {
		(
			"swaps.recovering",
			role_hint(view.role, "swaps.recovery_seller", "swaps.recovery_buyer"),
		)
	} else if let Some(packet) = view.message().filter(|p| copied != Some(p)) {
		match packet {
			Packet::Release { .. } => ("swaps.finish_title", "swaps.release_hint"),
			Packet::Round { message } if message.round == 0 => {
				("swaps.prepare_title", "swaps.send_offer_hint")
			}
			_ => ("swaps.prepare_title", "swaps.prepare_hint"),
		}
	} else if !view.ready {
		(
			"swaps.prepare_title",
			if importing {
				"swaps.reply_hint"
			} else {
				"swaps.reply_wait_hint"
			},
		)
	} else if !view.armed {
		(
			"swaps.pay_title",
			if view.role == Role::SellGrin {
				"swaps.start_hint"
			} else {
				"swaps.start_buy_hint"
			},
		)
	} else if importing {
		("swaps.finish_title", "swaps.release_receive_hint")
	} else if let Some(hint) = next(view) {
		(
			if phase == Some(2) {
				"swaps.finish_title"
			} else {
				"swaps.pay_title"
			},
			hint,
		)
	} else if view.payment().is_some() {
		("swaps.pay_title", "swaps.pay_hint")
	} else if phase == Some(2) {
		(
			"swaps.finish_title",
			if view.role == Role::SellGrin {
				"swaps.finish_wait_hint"
			} else {
				"swaps.release_receive_hint"
			},
		)
	} else {
		("swaps.pay_title", "swaps.payment_wait_hint")
	}
}

#[cfg(test)]
mod tests {
	use super::super::chain::explorer;
	use super::*;
	use std::time::Duration;
	fn view(action: Action) -> SwapView {
		let id = uuid::Uuid::new_v4();
		SwapView {
			id,
			role: Role::SellGrin,
			proposal: grin_wallet_api::swap::negotiation::Proposal {
				grin: 1_000_000_000,
				bitcoin: 100_000,
				fee: 12_500_000,
				chain: "Testnet".into(),
				network: "testnet4".into(),
				terms: grin_wallet_libwallet::swap::sas::Terms {
					revoke: 2000,
					refund: 2120,
					timeout: 2240,
					confirmations: 2,
					bitcoin_confirmations: 2,
					margin: 3,
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
			updated: None,
			reply: Some(grin_wallet_api::swap::Reply {
				grin: None,
				funding_recovery: false,
				chain: None,
				withdrawal: None,
				payout: None,
				id,
				action,
				proof: None,
				key: String::new(),
				main: None,
				funding: Some(String::new()),
				payment: None,
			}),
		}
	}

	fn chain_state(view: &mut SwapView) -> &mut grin_wallet_libwallet::swap::sas::View {
		&mut view.reply.as_mut().unwrap().chain.as_mut().unwrap().status
	}

	#[test]
	fn recovery_wait_without_bitcoin() {
		let mut v = view(Action::Wait);
		v.outgoing = None;
		assert_eq!(Status::new(&v, None, false).hint, "swaps.chain_wait");
		v.reply.as_mut().unwrap().grin = Some(grin_wallet_libwallet::swap::sas::GrinView {
			height: 2050,
			funding: TxState::Confirmed(100),
			success: TxState::Absent,
			revoke: TxState::Confirmed(2),
			refund: TxState::Absent,
			timeout: TxState::Absent,
			funded: false,
			revoked: true,
		});
		assert_eq!(Status::new(&v, None, false).hint, "swaps.recovery_seller");
		v.role = Role::BuyGrin;
		assert_eq!(Status::new(&v, None, false).hint, "swaps.recovery_buyer");
		v.reply.as_mut().unwrap().grin = None;
		assert_eq!(Status::new(&v, None, false).hint, "swaps.chain_wait");
	}

	#[test]
	fn missing_funding_offers_manual_recovery() {
		let mut v = view(Action::Refunded);
		v.role = Role::BuyGrin;
		v.reply.as_mut().unwrap().funding_recovery = true;
		assert!(!v.finished());
		let status = Status::new(&v, None, false);
		assert_eq!(status.title, "swaps.funding_recovery_title");
		assert_eq!(status.hint, "swaps.funding_recovery_hint");
		v.reply.as_mut().unwrap().funding_recovery = false;
		assert_eq!(Status::new(&v, None, false).hint, "swaps.withdraw_hint");
	}

	#[test]
	fn progress() {
		let mut view = view(Action::Wait);
		view.ready = false;
		assert_eq!(phase(&view), Some(0));
		view.ready = true;
		assert_eq!(phase(&view), Some(1));
		view.reply.as_mut().unwrap().action = Action::Complete;
		assert_eq!(phase(&view), Some(2));
		view.paid = true;
		assert_eq!(phase(&view), Some(3));
		for action in [Action::Refunded, Action::TimedOut, Action::RevokeGrin] {
			view.reply.as_mut().unwrap().action = action;
			assert_eq!(phase(&view), None);
		}
		view.reply.as_mut().unwrap().action = Action::Wait;
		view.stopped = true;
		assert_eq!(phase(&view), None);
		view.stopped = false;
		view.cancelled = true;
		assert_eq!(phase(&view), None);
	}
	#[test]
	fn instructions() {
		let mut v = view(Action::Wait);
		v.updated = Some(std::time::Instant::now());
		let chain = grin_wallet_api::swap::sas::Chain {
			kernels: Default::default(),
			status: grin_wallet_libwallet::swap::sas::View {
				height: 100,
				funding: TxState::Confirmed(2),
				success: TxState::Absent,
				revoke: TxState::Absent,
				refund: TxState::Absent,
				timeout: TxState::Absent,
				funded: true,
				revoked: false,
				bitcoin: TxState::Absent,
				bitcoin_started: false,
				bitcoin_unspent: false,
			},
			address: "contract".into(),
			network: "testnet4".into(),
			txid: None,
		};
		assert_eq!(
			explorer(&chain).as_deref(),
			Some("https://mempool.space/testnet4/address/contract")
		);
		v.reply.as_mut().unwrap().chain = Some(chain);
		assert_eq!(next(&v), Some("swaps.wait_buyer"));
		v.role = Role::BuyGrin;
		v.reply.as_mut().unwrap().action = Action::FundOther;
		v.reply.as_mut().unwrap().funding = None;
		v.reply.as_mut().unwrap().payment = Some(grin_wallet_api::swap::sas::Payment {
			address: "contract".into(),
			amount: 100_000,
			network: "testnet4".into(),
			expires: 1500,
		});
		assert_eq!(next(&v), Some("swaps.pay_hint"));
		for state in [TxState::Pending, TxState::Confirmed(1)] {
			chain_state(&mut v).bitcoin = state;
			assert_eq!(next(&v), Some("swaps.wait_bitcoin_buyer"));
			v.role = Role::SellGrin;
			assert_eq!(next(&v), Some("swaps.wait_bitcoin_seller"));
			v.role = Role::BuyGrin;
		}
		chain_state(&mut v).bitcoin = TxState::Confirmed(2);
		assert_eq!(next(&v), Some("swaps.need_release"));
		assert_eq!(phase(&v), Some(2));
		v.role = Role::SellGrin;
		assert_eq!(next(&v), Some("swaps.wait_claim"));
		v.reply.as_mut().unwrap().chain.as_mut().unwrap().txid = Some("funding".into());
		assert_eq!(
			explorer(v.chain().unwrap()).as_deref(),
			Some("https://mempool.space/testnet4/tx/funding")
		);
		v.reply.as_mut().unwrap().chain.as_mut().unwrap().network = "regtest".into();
		assert!(explorer(v.chain().unwrap()).is_none());
		v.updated = Some(std::time::Instant::now() - Duration::from_secs(11));
		assert!(v.chain().is_none());
		assert_eq!(phase(&v), Some(1));
		assert_eq!(next(&v), Some("swaps.chain_wait"));
		v.updated = Some(std::time::Instant::now());
		chain_state(&mut v).success = TxState::Pending;
		assert_eq!(next(&v), Some("swaps.wait_receipt_seller"));
		v.role = Role::BuyGrin;
		assert_eq!(next(&v), Some("swaps.wait_receipt_buyer"));
		chain_state(&mut v).revoke = TxState::Confirmed(2);
		assert_eq!(next(&v), Some("swaps.recovery_buyer"));
		v.role = Role::SellGrin;
		assert_eq!(next(&v), Some("swaps.recovery_seller"));
	}
}
