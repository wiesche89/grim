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

use crate::gui::{
	Colors,
	platform::PlatformCallbacks,
	views::{TextEdit, View},
};

pub(super) fn field(
	ui: &mut egui::Ui,
	id: &str,
	label: impl Into<egui::WidgetText>,
	value: &mut String,
	cb: &dyn PlatformCallbacks,
) {
	ui.label(label);
	TextEdit::new(ui.make_persistent_id(id))
		.focus(false)
		.paste()
		.ui(ui, value, cb);
	ui.add_space(6.0);
}

pub(super) fn button(ui: &mut egui::Ui, label: impl Into<String>) -> bool {
	let mut clicked = false;
	ui.with_layout(
		egui::Layout::top_down_justified(egui::Align::Center),
		|ui| {
			ui.spacing_mut().interact_size.y = 44.0;
			View::action_button(ui, label, || clicked = true);
		},
	);
	clicked
}

pub(super) fn secondary(ui: &mut egui::Ui, label: impl Into<String>) -> bool {
	let mut clicked = false;
	ui.with_layout(
		egui::Layout::top_down_justified(egui::Align::Center),
		|ui| {
			ui.spacing_mut().interact_size.y = 40.0;
			View::button(ui, label, Colors::white_or_black(false), || clicked = true);
		},
	);
	clicked
}

pub(super) fn action(ui: &mut egui::Ui, busy: bool, label: impl Into<String>) -> bool {
	ui.add_enabled_ui(!busy, |ui| button(ui, label)).inner
}

pub(super) fn panel(ui: &mut egui::Ui, draw: impl FnOnce(&mut egui::Ui)) {
	ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
		ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
		ui.spacing_mut().button_padding = egui::vec2(14.0, 8.0);
		draw(ui);
	});
}

pub(super) fn intro(
	ui: &mut egui::Ui,
	phase: Option<u8>,
	title: impl AsRef<str>,
	hint: impl Into<egui::WidgetText>,
) {
	ui.horizontal_wrapped(|ui| {
		ui.heading(title.as_ref());
		if let Some(step) = phase.filter(|s| *s < 3) {
			ui.label(
				egui::RichText::new(format!("{} / 3", step + 1))
					.small()
					.color(Colors::gray()),
			);
		}
	});
	if let Some(step) = phase {
		ui.add_space(6.0);
		ui.add(
			egui::ProgressBar::new((step + 1).min(3) as f32 / 3.0)
				.desired_height(4.0)
				.fill(Colors::gold()),
		);
	}
	ui.add_space(12.0);
	ui.add(egui::Label::new(hint).wrap());
	ui.add_space(12.0);
}
