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

use crate::gui::views::View;
use crate::{
	gui::{Colors, platform::PlatformCallbacks},
	wallet::{
		Wallet,
		swaps::{
			Command, Receipt, Service, Snapshot, SwapView, offer_fee, validate_address,
			validate_amounts,
		},
	},
};
use grin_wallet_api::swap::{negotiation::Message, pack::Packet};
use grin_wallet_libwallet::swap::{Action, Role, TxState};
use std::time::Duration;

mod amount;
mod chain;
mod widgets;
use amount::{coins, units};
use chain::{chain_status, deadlines, transactions};
use widgets::{action, button, field, intro, panel, secondary};
mod status;
use status::{Status, role_hint};

#[derive(PartialEq)]
enum Page {
	Chains,
	Home,
	Paste,
	Amount,
	Receive,
	Review,
}

enum Completion {
	Update,
	Import,
	Archive,
}

pub struct SwapContent {
	service: Service,
	grin: String,
	amount: String,
	address: String,
	incoming: String,
	copied: Option<Packet>,
	importing: bool,
	pending: Option<(Receipt, Completion)>,
	offer: Option<Message>,
	error: Option<String>,
	abort: bool,
	page: Page,
}

impl SwapContent {
	pub fn alive(&self) -> bool {
		self.service.alive()
	}

	fn send(&mut self, command: Command) {
		let completion = match &command {
			Command::Import(_) => Completion::Import,
			Command::Archive => Completion::Archive,
			_ => Completion::Update,
		};
		match self.service.send(command) {
			Ok(receipt) => {
				self.pending = Some((receipt, completion));
				self.error = None;
			}
			Err(error) => self.error = Some(error.to_string()),
		}
	}

	fn complete(&mut self) {
		let Some(result) = self.pending.as_ref().and_then(|(r, _)| r.result()) else {
			return;
		};
		let Some((_, completion)) = self.pending.take() else {
			return;
		};
		match result {
			Err(error) => self.error = Some(error),
			Ok(()) => match completion {
				Completion::Import => {
					self.incoming.clear();
					self.importing = false;
				}
				Completion::Archive => {
					self.page = Page::Chains;
					self.offer = None;
					self.copied = None;
					self.incoming.clear();
					self.importing = false;
					self.abort = false;
				}
				Completion::Update => (),
			},
		}
	}

	pub fn resume(&mut self, wallet: &Wallet) {
		self.service = wallet.swap_service();
	}

	pub fn new(wallet: &Wallet) -> Self {
		let service = wallet.swap_service();
		Self {
			service,
			grin: "1".into(),
			amount: "0.001".into(),
			address: String::new(),
			incoming: String::new(),
			copied: None,
			importing: false,
			pending: None,
			offer: None,
			error: None,
			abort: false,
			page: Page::Chains,
		}
	}

	pub fn ui(&mut self, ui: &mut egui::Ui, wallet: &Wallet, cb: &dyn PlatformCallbacks) {
		ui.push_id(wallet.account_id(), |ui| self.content_ui(ui, wallet, cb));
	}

	fn content_ui(&mut self, ui: &mut egui::Ui, wallet: &Wallet, cb: &dyn PlatformCallbacks) {
		ui.ctx().request_repaint_after(Duration::from_secs(1));
		egui::ScrollArea::vertical()
			.id_salt("swaps")
			.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
			.show(ui, |ui| {
				View::max_width_ui(ui, 460.0, |ui| {
					panel(ui, |ui| {
						ui.add_space(12.0);
						if !wallet.swaps_enabled() {
							ui.label(t!("swaps.testnet"));
							return;
						}
						let snapshot = self.service.snapshot();
						self.complete();
						if snapshot.view.is_none() {
							if self.page == Page::Chains {
								self.chains(ui);
								return;
							}
							if self.page == Page::Home
								&& ui.small_button(t!("swaps.choose_chain")).clicked()
							{
								self.page = Page::Chains;
								return;
							}
						}
						let status = &snapshot.connection;
						let (text, color) = if status.connected() {
							(t!("swaps.connected"), Colors::green())
						} else if status.error.is_some() {
							(t!("swaps.disconnected"), Colors::red())
						} else {
							(t!("swaps.connecting"), Colors::text(false))
						};
						ui.horizontal_wrapped(|ui| {
							ui.label(
								egui::RichText::new(format!(
									"Bitcoin: {text} ({})",
									snapshot.bitcoin.network
								))
								.size(14.0)
								.color(color),
							);
							if snapshot.busy {
								ui.add(egui::Spinner::new().size(12.0));
							}
						});
						ui.add_space(18.0);
						if let Some(error) = &self.error {
							ui.colored_label(Colors::red(), error);
						}
						if snapshot.error.is_some() {
							ui.colored_label(Colors::red(), t!("swaps.paused"));
							if ui
								.add_enabled(!snapshot.busy, egui::Button::new(t!("retry")))
								.clicked()
							{
								if self.service.alive() {
									self.send(Command::Retry);
								} else {
									self.service = wallet.retry_swaps();
									self.error = None;
								}
							}
						}
						egui::Frame::new()
							.fill(Colors::fill_lite())
							.corner_radius(12)
							.inner_margin(20)
							.stroke(egui::Stroke::new(1.0, Colors::stroke()))
							.show(ui, |ui| {
								if let Some(view) = &snapshot.view {
									self.session(
										ui,
										view,
										cb,
										snapshot.busy || self.pending.is_some(),
									);
								} else {
									self.setup(ui, &snapshot, cb);
								}
							});
						if let Some(view) = &snapshot.view {
							transactions(ui, view, cb);
							deadlines(ui, view);
						}
						ui.add_space(18.0);
						self.advanced(ui, &snapshot);
					});
				});
			});
	}

	fn chains(&mut self, ui: &mut egui::Ui) {
		ui.heading(t!("swaps.choose_chain"));
		ui.label(t!("swaps.choose_chain_hint"));
		ui.add_space(18.0);
		if button(ui, "Bitcoin (BTC)") {
			self.page = Page::Home;
		}
		ui.add_enabled_ui(false, |ui| {
			secondary(ui, "Ethereum (ETH)");
		});
		ui.small(t!("swaps.chain_unavailable"));
	}

	fn setup(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, cb: &dyn PlatformCallbacks) {
		match self.page {
			Page::Chains => return,
			Page::Home => {
				ui.vertical_centered(|ui| {
					ui.add_space(8.0);
					ui.label(
						egui::RichText::new("GRIN  ↔  BTC")
							.size(26.0)
							.color(Colors::title(false)),
					);
					ui.add_space(4.0);
					ui.label(
						egui::RichText::new(t!("swaps.slatepack_hint"))
							.size(15.0)
							.color(Colors::gray()),
					);
					ui.add_space(24.0);
				});
				if button(ui, t!("swaps.sell")) {
					self.offer = None;
					self.error = None;
					self.page = Page::Amount;
				}
				ui.add_space(10.0);
				if button(ui, t!("swaps.paste_offer")) {
					self.incoming.clear();
					self.error = None;
					self.page = Page::Paste;
				}
				ui.add_space(8.0);
			}
			Page::Paste => {
				intro(
					ui,
					Some(0),
					t!("swaps.prepare_title"),
					t!("swaps.offer_hint"),
				);
				self.inbox(ui, cb, true, snapshot.busy);
			}
			Page::Amount => {
				intro(
					ui,
					Some(0),
					t!("swaps.prepare_title"),
					t!("swaps.amount_hint"),
				);
				ui.add_space(12.0);
				field(ui, "grin", t!("swaps.give"), &mut self.grin, cb);
				field(ui, "bitcoin", t!("swaps.get"), &mut self.amount, cb);
				if button(ui, t!("continue")) {
					if units(&self.grin, 9)
						.zip(units(&self.amount, 8))
						.is_some_and(|(grin, bitcoin)| validate_amounts(grin, bitcoin))
					{
						self.error = None;
						self.page = Page::Receive;
					} else {
						self.error = Some(t!("swaps.amount_error").into());
					}
				}
			}
			Page::Receive => {
				intro(
					ui,
					Some(0),
					t!("swaps.receive_step"),
					t!("swaps.address_hint"),
				);
				ui.add_space(12.0);
				if let Some(offer) = &self.offer {
					ui.label(format!(
						"{} GRIN ↔ {} BTC",
						coins(offer.proposal.grin - offer.proposal.fee, 9),
						coins(offer.proposal.bitcoin, 8)
					));
				}
				field(
					ui,
					"address",
					t!("swaps.address_short"),
					&mut self.address,
					cb,
				);
				ui.add_space(12.0);
				if button(ui, t!("continue")) {
					let valid = validate_address(&snapshot.bitcoin.network, &self.address);
					if !valid {
						self.error = Some(t!("swaps.address_error").into());
					} else {
						self.error = None;
						self.page = Page::Review;
					}
				}
			}
			Page::Review => {
				intro(ui, Some(0), t!("swaps.review"), t!("swaps.review_hint"));
				ui.add_space(12.0);
				if let Some(offer) = &self.offer {
					ui.label(format!(
						"{} GRIN ↔ {} BTC",
						coins(offer.proposal.grin - offer.proposal.fee, 9),
						coins(offer.proposal.bitcoin, 8)
					));
				} else {
					ui.label(format!("{} GRIN ↔ {} BTC", self.grin, self.amount));
				}
				ui.small(t!("swaps.extra_fees"));
				ui.add_space(8.0);
				ui.label(t!("swaps.address_short"));
				ui.add(egui::Label::new(self.address.trim()).wrap());
				ui.add_space(12.0);
				ui.add_enabled_ui(!snapshot.busy && snapshot.connection.connected(), |ui| {
					let text = if self.offer.is_some() {
						t!("swaps.accept_short")
					} else {
						t!("swaps.create_short")
					};
					if button(ui, text) {
						if let Some(offer) = &self.offer {
							self.send(Command::Accept {
								expected: offer.clone(),
								address: self.address.trim().into(),
							});
						} else if let (Some(grin), Some(bitcoin)) = (
							units(&self.grin, 9).and_then(|g| g.checked_add(offer_fee())),
							units(&self.amount, 8),
						) {
							self.send(Command::Create {
								grin,
								bitcoin,
								address: self.address.trim().into(),
							});
						}
					}
				});
			}
		}
		if self.page != Page::Home {
			ui.add_space(8.0);
			if ui.button(t!("back")).clicked() {
				self.error = None;
				self.page = match self.page {
					Page::Review => Page::Receive,
					Page::Receive if self.offer.is_none() => Page::Amount,
					_ => Page::Home,
				};
			}
		}
	}

	fn inbox(&mut self, ui: &mut egui::Ui, cb: &dyn PlatformCallbacks, offer: bool, busy: bool) {
		ui.add_space(10.0);
		ui.add_enabled(
			self.pending.is_none(),
			egui::TextEdit::multiline(&mut self.incoming)
				.desired_width(f32::INFINITY)
				.desired_rows(4)
				.font(egui::TextStyle::Monospace)
				.char_limit(4 * 1024 * 1024)
				.hint_text("BEGINSLATEPACK. … . ENDSLATEPACK."),
		);
		ui.add_space(8.0);
		if ui
			.add_enabled(self.pending.is_none(), egui::Button::new(t!("swaps.paste")))
			.clicked()
		{
			self.incoming = cb.get_string_from_buffer();
		}
		ui.add_space(8.0);
		ui.add_enabled_ui(
			!busy && self.pending.is_none() && !self.incoming.trim().is_empty(),
			|ui| {
				if button(ui, t!("continue")) {
					if offer {
						match Packet::offer(&self.incoming) {
							Ok(offer) => {
								self.incoming.clear();
								self.offer = Some(offer);
								self.error = None;
								self.page = Page::Receive;
							}
							Err(_) => self.error = Some(t!("swaps.pack_error").into()),
						}
					} else {
						self.send(Command::Import(self.incoming.clone()));
					}
				}
			},
		);
		ui.horizontal(|ui| {
			ui.set_min_height(22.0);
			if self.pending.is_some() {
				ui.add(egui::Spinner::new().size(16.0));
				ui.label(t!("swaps.checking_message"));
			}
		});
	}

	fn messages(
		&mut self,
		ui: &mut egui::Ui,
		view: &SwapView,
		cb: &dyn PlatformCallbacks,
		busy: bool,
	) {
		if view.cancelled || view.finished() || view.stopped || view.owns_bitcoin() {
			return;
		}
		let packet = view.message();
		let copied = packet.is_some() && self.copied.as_ref() == packet;
		if !self.importing
			&& let Some(packet) = packet
		{
			let clicked = if copied {
				secondary(ui, t!("swaps.copy_again"))
			} else {
				button(ui, t!("swaps.copy_pack"))
			};
			if clicked {
				match packet.encode() {
					Ok(text) => {
						cb.copy_string_to_buffer(text);
						self.copied = Some(packet.clone());
					}
					Err(e) => self.error = Some(e.to_string()),
				}
			}
		}
		if !view.ready || (view.armed && view.role == Role::BuyGrin) {
			if self.importing {
				self.inbox(ui, cb, false, busy);
				if ui
					.add_enabled(
						self.pending.is_none(),
						egui::Button::new(t!("back")).small(),
					)
					.clicked()
				{
					self.incoming.clear();
					self.importing = false;
				}
			} else {
				let label = if view.ready {
					t!("swaps.paste_release")
				} else {
					t!("swaps.paste_reply")
				};
				let primary = !view.ready && (packet.is_none() || copied);
				let clicked = if primary {
					button(ui, label)
				} else {
					secondary(ui, label)
				};
				if clicked {
					self.importing = true;
				}
			}
		}
	}

	fn session(
		&mut self,
		ui: &mut egui::Ui,
		view: &SwapView,
		cb: &dyn PlatformCallbacks,
		busy: bool,
	) {
		let p = &view.proposal;
		let status = Status::new(view, self.copied.as_ref(), self.importing);
		let pending = view
			.message()
			.is_some_and(|p| matches!(p, Packet::Round { .. }) && self.copied.as_ref() != Some(p));
		let preparation = if pending || !view.ready {
			view.outgoing.as_ref().and_then(|packet| match packet {
				Packet::Round { message } => Some((message.round + 1, 4)),
				_ => None,
			})
		} else {
			None
		};
		intro(ui, status.phase, t!(status.title), t!(status.hint));
		if let Some((current, total)) = preparation {
			ui.label(t!(
				"swaps.preparation_count",
				current = current,
				total = total
			));
			ui.add_space(12.0);
		}
		ui.label(format!(
			"{} GRIN ↔ {} BTC",
			coins(p.grin - p.fee, 9),
			coins(p.bitcoin, 8)
		));
		ui.small(t!(role_hint(view.role, "swaps.selling", "swaps.buying")));
		ui.add_space(12.0);
		chain_status(ui, view);
		if view.ready && !view.cancelled {
			if !view.armed && !view.stopped {
				if !pending
					&& action(
						ui,
						busy,
						if view.role == Role::SellGrin {
							t!("swaps.start")
						} else {
							t!("swaps.start_buy")
						},
					) {
					self.send(Command::Start);
				}
			} else {
				if let Some(payment) = view.payment() {
					ui.add_space(12.0);

					ui.add(
						egui::Label::new(egui::RichText::new(&payment.address).monospace()).wrap(),
					);
					if secondary(ui, t!("swaps.copy_address")) {
						cb.copy_string_to_buffer(payment.address.clone());
					}
					if ui
						.button(format!("{} BTC ({})", coins(payment.amount, 8), t!("copy")))
						.clicked()
					{
						cb.copy_string_to_buffer(coins(payment.amount, 8));
					}
				}
			}
		}
		if !self.abort {
			self.messages(ui, view, cb, busy);
		}
		if view.ready && !view.finished() && !view.stopped && !view.owns_bitcoin() {
			ui.add_space(12.0);
			ui.separator();
			if ui
				.add_enabled(
					self.pending.is_none(),
					egui::Button::new(t!("swaps.abort")).small(),
				)
				.clicked()
			{
				self.abort = true;
			}
			if self.abort {
				ui.label(t!("swaps.abort_short"));
				if action(ui, busy, t!("swaps.confirm_abort")) {
					self.send(Command::Abort);
					self.abort = false;
				}
				if ui.button(t!("close")).clicked() {
					self.abort = false;
				}
			}
		}
		if !view.ready && !view.cancelled {
			ui.add_space(12.0);
			if ui
				.add_enabled(
					!busy,
					egui::Button::new(t!("swaps.cancel_preparation")).small(),
				)
				.clicked()
			{
				self.send(Command::Abort);
			}
		}
		if view.finished() && action(ui, busy, t!("swaps.done")) {
			self.send(Command::Archive);
		}
	}

	fn advanced(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
		egui::CollapsingHeader::new(t!("swaps.advanced"))
			.id_salt("swap_advanced")
			.default_open(false)
			.show(ui, |ui| {
				if let Some(error) = snapshot
					.error
					.as_ref()
					.or(snapshot.connection.error.as_ref())
				{
					ui.label(error);
				}
				if let Some(height) = snapshot.connection.height {
					ui.small(format!("Bitcoin: {height}"));
				}
				if let Some(view) = &snapshot.view {
					ui.label(view.id.to_string());
					ui.small(format!("{}: {}", t!("swaps.round"), view.round));

					ui.label(format!("{}: {}", t!("swaps.address_short"), view.address));
				}
			});
	}
}
#[cfg(test)]
mod tests;
