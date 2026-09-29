use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

pub struct A64Abi;

impl A64Abi {
    pub fn arg(context: &touchHLE_DynarmicA64Context, index: usize) -> u64 {
        context.regs[index]
    }

    pub fn set_return(context: &mut touchHLE_DynarmicA64Context, value: u64) {
        context.regs[0] = value;
    }

    pub fn set_return_pair(context: &mut touchHLE_DynarmicA64Context, low: u64, high: u64) {
        context.regs[0] = low;
        context.regs[1] = high;
    }
}

#[cfg(test)]
mod tests {
    use super::A64Abi;
    use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

    #[test]
    fn reads_register_arguments_and_writes_return_register() {
        let mut context = touchHLE_DynarmicA64Context::default();
        context.regs[0] = 41;
        assert_eq!(A64Abi::arg(&context, 0), 41);
        A64Abi::set_return(&mut context, 42);
        assert_eq!(context.regs[0], 42);
    }
}
