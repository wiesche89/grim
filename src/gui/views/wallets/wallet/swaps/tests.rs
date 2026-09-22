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

#[test]
fn spacing() {
	for width in [320.0, 460.0] {
		let ctx = egui::Context::default();
		crate::setup_fonts(&ctx);
		let input = egui::RawInput {
			screen_rect: Some(egui::Rect::from_min_size(
				egui::Pos2::ZERO,
				egui::vec2(width, 800.0),
			)),
			..Default::default()
		};
		let _ = ctx.run(input, |ctx| {
			egui::CentralPanel::default().show(ctx, |ui| {
				ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
				panel(ui, |ui| {
					button(ui, "Copy message");
					let bottom = ui.min_rect().bottom();
					let reply = ui.button("Already have a reply?");
					assert!(reply.rect.top() - bottom >= 12.0);
					let cancel = ui.button("Cancel preparation");
					assert!(cancel.rect.top() - reply.rect.bottom() >= 12.0);
					assert!(cancel.rect.right() <= ui.max_rect().right());
				});
			});
		});
	}
}
