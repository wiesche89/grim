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

pub(super) fn units(value: &str, decimals: u32) -> Option<u64> {
	let value = value.trim();
	let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
	if whole.is_empty()
		|| !whole.bytes().all(|c| c.is_ascii_digit())
		|| fraction.len() > decimals as usize
		|| !fraction.bytes().all(|c| c.is_ascii_digit())
	{
		return None;
	}
	let base = whole
		.parse::<u64>()
		.ok()?
		.checked_mul(10u64.pow(decimals))?;
	let part = if fraction.is_empty() {
		0
	} else {
		fraction
			.parse::<u64>()
			.ok()?
			.checked_mul(10u64.pow(decimals - fraction.len() as u32))?
	};
	base.checked_add(part)
}

pub(super) fn coins(amount: u64, decimals: u32) -> String {
	let unit = 10u64.pow(decimals);
	format!(
		"{}.{:0width$}",
		amount / unit,
		amount % unit,
		width = decimals as usize
	)
	.trim_end_matches('0')
	.trim_end_matches('.')
	.to_owned()
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn amounts() {
		assert_eq!(units("0.001", 8), Some(100_000));
		assert_eq!(units("1.000000001", 9), Some(1_000_000_001));
		for value in ["-1", "1e3", "0.000000001", "1,2", "18446744073709551616"] {
			assert_eq!(units(value, 8), None);
		}
		assert_eq!(coins(100_000, 8), "0.001");
		assert_eq!(coins(1_000_000_000, 9), "1");
		assert_eq!(coins(1, 8), "0.00000001");
		assert_eq!(coins(0, 8), "0");
	}
}
