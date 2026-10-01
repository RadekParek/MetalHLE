/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `task`

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::mach::arm::vm_types::vm_size_t;
use crate::libc::mach::core_types::{integer_t, natural_t};
use crate::libc::mach::init::MACH_TASK_SELF;
use crate::libc::mach::policy::policy_t;
use crate::libc::mach::port::mach_port_t;
use crate::libc::mach::thread_info::{
    kern_return_t, mach_msg_type_number_t, KERN_INVALID_ARGUMENT, KERN_INVALID_TASK, KERN_SUCCESS,
};
use crate::libc::mach::time_value::time_value_t;
use crate::mem::{guest_size_of, MutPtr, SafeRead};
use crate::Environment;

#[repr(C, packed)]
struct task_basic_info {
    suspend_count: integer_t,
    virtual_size: vm_size_t,
    resident_size: vm_size_t,
    user_time: time_value_t,
    system_time: time_value_t,
    policy: policy_t,
}
unsafe impl SafeRead for task_basic_info {}

#[allow(non_camel_case_types)]
type task_name_t = mach_port_t;
#[allow(non_camel_case_types)]
type task_flavor_t = natural_t;
#[allow(non_camel_case_types)]
type task_info_t = MutPtr<integer_t>;

const TASK_BASIC_INFO: task_flavor_t = 4;

#[allow(dead_code)]
const POLICY_NULL: policy_t = 0;
const POLICY_TIMESHARE: policy_t = 1;
#[allow(dead_code)]
const POLICY_RR: policy_t = 2;
#[allow(dead_code)]
const POLICY_FIFO: policy_t = 4;

fn validate_task_info_request(
    target_task: task_name_t,
    flavor: task_flavor_t,
    out_size_available: mach_msg_type_number_t,
) -> Result<mach_msg_type_number_t, kern_return_t> {
    if target_task != MACH_TASK_SELF {
        return Err(KERN_INVALID_TASK);
    }
    if flavor != TASK_BASIC_INFO {
        return Err(KERN_INVALID_ARGUMENT);
    }
    let expected = (guest_size_of::<task_basic_info>() / guest_size_of::<integer_t>())
        as mach_msg_type_number_t;
    if expected > out_size_available {
        Err(KERN_INVALID_ARGUMENT)
    } else {
        Ok(expected)
    }
}

fn task_info(
    env: &mut Environment,
    target_task: task_name_t,
    flavor: task_flavor_t,
    task_info_out: task_info_t,
    task_info_out_cnt: MutPtr<mach_msg_type_number_t>,
) -> kern_return_t {
    log_dbg!(
        "task_info({:?}, {:?}, {:?}, {:?})",
        target_task,
        flavor,
        task_info_out,
        task_info_out_cnt
    );
    if task_info_out.is_null() || task_info_out_cnt.is_null() {
        return KERN_INVALID_ARGUMENT;
    }
    let out_size_available = env.mem.read(task_info_out_cnt);
    let out_size_expected =
        match validate_task_info_request(target_task, flavor, out_size_available) {
            Ok(count) => count,
            Err(error) => {
                log!(
                    "Warning: task_info({:?}, flavor={}, count={}): returning Mach error {}.",
                    target_task,
                    flavor,
                    out_size_available,
                    error
                );
                return error;
            }
        };
    // Per Apple documentation, write back the number of elements actually returned.
    env.mem.write(task_info_out_cnt, out_size_expected);
    // Values taken from an iPod Touch 4 running iOS 6.1
    env.mem.write(
        task_info_out.cast(),
        task_basic_info {
            suspend_count: 0,
            virtual_size: 280719360,
            resident_size: 2678784,
            user_time: time_value_t {
                seconds: 0,
                microseconds: 0,
            },
            system_time: time_value_t {
                seconds: 0,
                microseconds: 0,
            },
            policy: POLICY_TIMESHARE,
        },
    );
    KERN_SUCCESS
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(task_info(_, _, _, _))];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_info_rejects_non_self_task_without_panicking() {
        assert_eq!(
            validate_task_info_request(MACH_TASK_SELF.wrapping_add(1), TASK_BASIC_INFO, u32::MAX,),
            Err(KERN_INVALID_TASK)
        );
    }

    #[test]
    fn task_info_rejects_unsupported_flavors() {
        assert_eq!(
            validate_task_info_request(MACH_TASK_SELF, TASK_BASIC_INFO + 1, u32::MAX),
            Err(KERN_INVALID_ARGUMENT)
        );
    }

    #[test]
    fn task_info_rejects_undersized_output_buffers() {
        let expected = (guest_size_of::<task_basic_info>() / guest_size_of::<integer_t>())
            as mach_msg_type_number_t;
        assert_eq!(
            validate_task_info_request(MACH_TASK_SELF, TASK_BASIC_INFO, expected - 1),
            Err(KERN_INVALID_ARGUMENT)
        );
    }

    #[test]
    fn task_info_accepts_a_valid_basic_info_request() {
        let expected = (guest_size_of::<task_basic_info>() / guest_size_of::<integer_t>())
            as mach_msg_type_number_t;
        assert_eq!(
            validate_task_info_request(MACH_TASK_SELF, TASK_BASIC_INFO, expected),
            Ok(expected)
        );
    }
}
