// SPDX-FileCopyrightText: 2025 Rivos Inc.
//
// SPDX-License-Identifier: Apache-2.0

.section .text.custom_reset_entry, "ax"
.global my_custom_reset
my_custom_reset:
	jr ra
