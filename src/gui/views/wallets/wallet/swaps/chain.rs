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

use crate::{
	gui::{Colors, platform::PlatformCallbacks},
	wallet::swaps::SwapView,
};
use grin_wallet_libwallet::swap::{Action, TxState};

pub(super) fn explorer(chain: &grin_wallet_api::swap::sas::Chain) -> Option<String> {
	let (kind, value) = match &chain.txid {
		Some(id) => ("tx", id.as_str()),
		None => ("address", chain.address.as_str()),
	};
	crate::wallet::swaps::explorer_url(&chain.network, kind, value)
}

pub(super) fn confirmations(
	ui: &mut egui::Ui,
	name: &str,
	state: TxState,
	required: u64,
	bitcoin: bool,
	fresh: bool,
) {
	let text = match state {
		TxState::Confirmed(n) => t!(
			"swaps.confirmations",
			current = n.min(required),
			required = required
		)
		.into_owned(),
		TxState::Pending => t!("swaps.pending_confirmations", required = required).into_owned(),
		TxState::Absent => if bitcoin {
			t!("swaps.no_bitcoin")
		} else {
			t!("swaps.no_grin")
		}
		.into_owned(),
		TxState::Conflicted => t!("swaps.conflicted").into_owned(),
	};
	ui.label(egui::RichText::new(name).small().color(Colors::gray()));
	let color = if !fresh {
		Colors::gray()
	} else {
		match state {
			TxState::Confirmed(n) if n >= required => Colors::green(),
			TxState::Confirmed(_) | TxState::Pending | TxState::Conflicted => Colors::red(),
			TxState::Absent => Colors::text(false),
		}
	};
	ui.colored_label(color, text);
}

pub(super) fn deadlines(ui: &mut egui::Ui, view: &SwapView) {
	if view.finished() {
		return;
	}
	let terms = view.proposal.terms;
	let height = view.chain().map(|chain| chain.status.height);
	egui::CollapsingHeader::new(t!("swaps.deadline_title"))
		.id_salt(("swap_deadlines", view.id))
		.show(ui, |ui| {
			ui.small(t!(
				"swaps.finish_before",
				height = terms.revoke.saturating_sub(terms.margin)
			));
			for (label, deadline) in [
				("swaps.revoke_at", terms.revoke),
				("swaps.refund_at", terms.refund),
				("swaps.timeout_at", terms.timeout),
			] {
				let text = t!(label, height = deadline);
				match height {
					Some(height) if height >= deadline => {
						ui.colored_label(
							Colors::red(),
							format!("{text} ({})", t!("swaps.height_reached")),
						);
					}
					Some(height) => {
						ui.label(format!(
							"{text} ({})",
							t!("swaps.blocks_left", count = deadline - height)
						));
					}
					None => {
						ui.label(text);
					}
				}
			}
			if let Some(height) = height {
				ui.small(t!("swaps.grin_height", height = height));
			}
			ui.small(t!("swaps.height_hint"));
		});
}

pub(super) fn chain_status(ui: &mut egui::Ui, view: &SwapView) {
	if !view.ready || view.cancelled {
		return;
	}
	if let Some(chain) = view.last_chain() {
		let fresh = view.chain().is_some();
		if !fresh {
			ui.small(t!("swaps.previous_status"));
		}
		let terms = view.proposal.terms;
		confirmations(
			ui,
			&t!("swaps.grin_deposit"),
			chain.status.funding,
			terms.confirmations,
			false,
			fresh,
		);
		if chain.status.bitcoin_started && chain.status.bitcoin == TxState::Absent {
			ui.label(format!(
				"{}: {}",
				t!("swaps.bitcoin_deposit"),
				t!("swaps.bitcoin_used")
			));
		} else {
			confirmations(
				ui,
				&t!("swaps.bitcoin_deposit"),
				chain.status.bitcoin,
				terms.bitcoin_confirmations,
				true,
				fresh,
			);
		}
		if chain.status.success != TxState::Absent
			|| view
				.reply
				.as_ref()
				.is_some_and(|r| r.action == Action::ClaimGrin)
		{
			confirmations(
				ui,
				&t!("swaps.grin_receipt"),
				chain.status.success,
				terms.confirmations,
				false,
				fresh,
			);
		}
		for (label, state) in [
			("swaps.revoke_status", chain.status.revoke),
			("swaps.refund_status", chain.status.refund),
			("swaps.timeout_status", chain.status.timeout),
		] {
			if state != TxState::Absent {
				confirmations(ui, &t!(label), state, terms.confirmations, false, fresh);
			}
		}
	}

	ui.add_space(12.0);
}

pub(super) fn transactions(ui: &mut egui::Ui, view: &SwapView, cb: &dyn PlatformCallbacks) {
	let Some(chain) = view.last_chain() else {
		return;
	};
	egui::CollapsingHeader::new(t!("swaps.transactions"))
		.id_salt(("swap_transactions", view.id))
		.show(ui, |ui| {
			if let Some(url) = explorer(chain) {
				if ui.link(t!("swaps.mempool")).clicked() {
					ui.ctx().open_url(egui::OpenUrl { url, new_tab: true });
				}
				ui.add_space(8.0);
			}
			for (label, id) in [
				("swaps.bitcoin_deposit", chain.txid.as_ref()),
				(
					"swaps.bitcoin_payout",
					view.reply.as_ref().and_then(|r| r.withdrawal.as_ref()),
				),
			] {
				if let Some(id) = id {
					reference(ui, &t!(label), id, cb);
				}
			}
			for (kind, id) in &chain.kernels {
				let label = match kind.as_str() {
					"funding" => "swaps.grin_deposit",
					"success" => "swaps.grin_receipt",
					"revoke" => "swaps.revoke_status",
					"refund" => "swaps.refund_status",
					"timeout" => "swaps.timeout_status",
					_ => continue,
				};
				reference(ui, &format!("{} (Kernel)", t!(label)), id, cb);
			}
		});
}

pub(super) fn reference(ui: &mut egui::Ui, label: &str, id: &str, cb: &dyn PlatformCallbacks) {
	ui.label(label);
	ui.add(egui::Label::new(egui::RichText::new(id).monospace()).wrap());
	if ui.small_button(t!("copy")).clicked() {
		cb.copy_string_to_buffer(id.to_owned());
	}
	ui.add_space(8.0);
}
