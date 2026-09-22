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
	gui::{
		Colors,
		platform::PlatformCallbacks,
		views::{TextEdit, View},
	},
	wallet::{
		Wallet,
		swaps::{Command, NETWORKS, Receipt, Timing, server},
	},
};
use grin_wallet_config::types::BitcoinConfig;

#[derive(Default)]
pub struct SwapSettings {
	config: Option<BitcoinConfig>,
	timing: Timing,
	cookie: String,
	proxy: String,
	pending: Option<Receipt>,
	error: Option<String>,
	saved: bool,
}

impl SwapSettings {
	pub fn ui(&mut self, ui: &mut egui::Ui, wallet: &Wallet, cb: &dyn PlatformCallbacks) {
		ui.vertical_centered(|ui| {
			ui.spacing_mut().item_spacing.y = 8.0;
			self.content_ui(ui, wallet, cb);
		});
	}

	fn content_ui(&mut self, ui: &mut egui::Ui, wallet: &Wallet, cb: &dyn PlatformCallbacks) {
		ui.add_space(16.0);
		ui.separator();
		ui.heading(t!("swaps.title"));
		if !wallet.swaps_enabled() {
			ui.label(t!("swaps.testnet"));
			return;
		}
		if !wallet.is_open() || wallet.is_closing() {
			return;
		}
		let service = wallet.swap_service();
		let snapshot = service.snapshot();
		if !service.alive() {
			if let Some(error) = &snapshot.error {
				ui.colored_label(Colors::red(), error);
			}
			View::action_button(ui, t!("retry"), || {
				wallet.retry_swaps();
			});
			return;
		}
		if let Some(result) = self.pending.as_ref().and_then(Receipt::result) {
			self.pending = None;
			match result {
				Ok(()) => {
					self.saved = true;
					self.error = None;
				}
				Err(error) => {
					self.saved = false;
					self.error = Some(error);
				}
			}
		}
		if self.config.is_none() && !snapshot.busy {
			self.cookie = snapshot.bitcoin.cookie.to_string_lossy().into_owned();
			self.proxy = snapshot.bitcoin.proxy.clone().unwrap_or_default();
			self.config = Some(snapshot.bitcoin.clone());
			self.timing = snapshot.timing;
		}
		let Some(config) = self.config.as_mut() else {
			ui.spinner();
			return;
		};
		let locked = snapshot.view.is_some();
		if locked {
			ui.label(t!("swaps.settings_locked"));
		}
		ui.push_id(("swap_settings", wallet.identifier()), |ui| {
			ui.add_enabled_ui(!locked && !snapshot.busy && self.pending.is_none(), |ui| {
				let mut network_changed = false;
				ui.label(t!("swaps.network"));
				let width = ui.available_width().min(180.0);
				ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
					egui::ComboBox::from_id_salt("network")
						.width(width)
						.selected_text(&config.network)
						.show_ui(ui, |ui| {
							ui.add_enabled(false, egui::Button::new("Mainnet"));
							for network in NETWORKS {
								network_changed |= ui
									.selectable_value(
										&mut config.network,
										network.name.into(),
										network.label,
									)
									.changed();
							}
						});
				});
				ui.small(t!("swaps.mainnet_unavailable"));
				let mut remote = config.url.starts_with("https://");
				let was_remote = remote;
				ui.add_space(4.0);
				let width = ui.available_width().min(360.0);
				ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
					ui.columns(2, |columns| {
						columns[0].vertical_centered(|ui| {
							ui.add_enabled_ui(config.network != "regtest", |ui| {
								ui.selectable_value(&mut remote, true, t!("swaps.public"));
							});
						});
						columns[1].vertical_centered(|ui| {
							ui.selectable_value(&mut remote, false, t!("swaps.local"));
						});
					});
				});
				ui.add_space(4.0);
				if config.network == "regtest" {
					remote = false;
				}
				if (network_changed || was_remote != remote)
					&& let Some(url) = server(&config.network, remote)
				{
					config.url = url;
				}
				ui.label(t!("swaps.server"));
				TextEdit::new(ui.make_persistent_id("server"))
					.focus(false)
					.paste()
					.ui(ui, &mut config.url, cb);
				if remote {
					ui.small(t!("swaps.remote_trust"));
					ui.label(t!("swaps.proxy"));
					TextEdit::new(ui.make_persistent_id("proxy"))
						.focus(false)
						.paste()
						.ui(ui, &mut self.proxy, cb);
					ui.small(t!("swaps.proxy_hint"));
				} else {
					ui.label(t!("swaps.cookie"));
					TextEdit::new(ui.make_persistent_id("cookie"))
						.focus(false)
						.paste()
						.ui(ui, &mut self.cookie, cb);
				}
				if config.url != snapshot.bitcoin.url
					|| config.network != snapshot.bitcoin.network
					|| self.cookie.trim() != snapshot.bitcoin.cookie.to_string_lossy()
					|| (remote && !self.proxy.trim().is_empty()).then_some(self.proxy.trim())
						!= snapshot.bitcoin.proxy.as_deref()
				{
					self.saved = false;
				}
				ui.add_space(8.0);
				View::action_button(ui, t!("swaps.connect"), || {
					config.url = config.url.trim().to_owned();
					self.cookie = self.cookie.trim().to_owned();
					self.proxy = self.proxy.trim().to_owned();
					let mut config = config.clone();
					config.cookie = self.cookie.trim().into();
					config.proxy = (remote && !self.proxy.trim().is_empty())
						.then(|| self.proxy.trim().to_owned());
					match service.send(Command::Configure(config)) {
						Ok(receipt) => {
							self.pending = Some(receipt);
							self.error = None;
							self.saved = false;
						}
						Err(error) => self.error = Some(error.to_string()),
					}
				});
			});
		});
		ui.add_space(12.0);
		ui.label(t!("swaps.timing"));
		ui.small(t!("swaps.timing_hint"));
		ui.add_enabled_ui(!locked && !snapshot.busy && self.pending.is_none(), |ui| {
			let before = self.timing;
			for (label, value) in [
				("swaps.revoke_delay", &mut self.timing.revoke),
				("swaps.refund_delay", &mut self.timing.refund),
				("swaps.timeout_delay", &mut self.timing.timeout),
				("swaps.grin_confirmations", &mut self.timing.confirmations),
				(
					"swaps.bitcoin_confirmations",
					&mut self.timing.bitcoin_confirmations,
				),
				("swaps.safety_margin", &mut self.timing.margin),
			] {
				ui.label(t!(label));
				ui.add(egui::DragValue::new(value).range(1..=u64::MAX).speed(1));
			}
			if before != self.timing {
				self.saved = false;
			}
			let valid = self.timing.validate().is_ok();
			if !valid {
				ui.colored_label(Colors::red(), t!("swaps.timing_invalid"));
			}
			ui.add_space(8.0);
			ui.add_enabled_ui(valid, |ui| {
				View::action_button(ui, t!("swaps.save_timing"), || {
					match service.send(Command::SetTiming(self.timing)) {
						Ok(receipt) => {
							self.pending = Some(receipt);
							self.error = None;
							self.saved = false;
						}
						Err(error) => self.error = Some(error.to_string()),
					}
				});
			});
		});
		if self.pending.is_some() {
			ui.spinner();
			ui.ctx()
				.request_repaint_after(std::time::Duration::from_millis(100));
		}
		if self.saved {
			ui.colored_label(Colors::green(), t!("swaps.settings_saved"));
		}
		if let Some(error) = &self.error {
			ui.colored_label(Colors::red(), error);
		}
		ui.add_space(16.0);
	}
}
