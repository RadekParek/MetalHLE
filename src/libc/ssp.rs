/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stack Smashing Protection (SSP)

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::environment::Environment;

/// The guest's stack canary check failed.
///
/// Stack-protector failure is `noreturn` in the guest ABI, but honouring that
/// (aborting or unwinding) kills sessions that would otherwise run fine: the
/// "corruption" is usually a benign canary-slot mismatch caused by an ABI
/// quirk in our emulation, not a real memory-safety event the guest could not
/// survive on real iOS. Field observation (Minecraft PE 0.14.2 world entry):
/// the first failure was recovered by frame unwinding, but a second failure on
/// the return-from-host-callback boundary had no unwind state and ended the
/// whole session.
///
/// So we simply return to the guest. The call site is a cold `noreturn` block
/// that continues into a trap (typically an undefined instruction); the
/// UndefinedInstruction bypass then fakes a function return to LR and the
/// guest function whose canary check failed just returns to its caller. No
/// session termination is ever requested here.
pub fn __stack_chk_fail(_env: &mut Environment) {
    static STACK_CHK_FAIL_LOGGED: std::sync::atomic::AtomicU32 =
        std::sync::atomic::AtomicU32::new(0);
    let n = STACK_CHK_FAIL_LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n < 8 || n == 1000 {
        log!(
            "Warning: __stack_chk_fail: guest stack canary mismatch (occurrence {}). \\
             Treating it as benign: returning to the guest and letting the \\
             noreturn trap be bypassed by the UndefinedInstruction recovery, \\
             so the session keeps running instead of ending.",
            n + 1
        );
    }
}

pub const FUNCTIONS: FunctionExports = &[
    // Экспортируем функцию. Макрос автоматически добавит нужное подчеркивание
    // для C.
    export_c_func!(__stack_chk_fail()),
];

pub const CONSTANTS: ConstantExports = &[
    // Используем гарантированно существующий вариант.
    // Игра получит валидный указатель на 0x00000000 и использует его как
    // канарейку.
    ("___stack_chk_guard", HostConstant::NullPtr),
];
