use super::{
    c_string, objc_field, objc_number, objc_number_float, objc_object, objc_text, set_objc_field,
    A64_KIND_ARRAY, A64_KIND_DATA, A64_KIND_DICTIONARY, A64_KIND_GENERIC, A64_KIND_MUTABLE_ARRAY,
    A64_KIND_MUTABLE_DICTIONARY, A64_KIND_MUTABLE_STRING, A64_KIND_STRING,
};
use crate::mem64::Mem64;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

#[derive(Clone, Copy, PartialEq, Eq)]
enum StubKind {
    ArrayCreate(bool),
    ArrayCount,
    ArrayValue,
    ArrayGetValues,
    ArrayFirstIndex,
    ArrayContains,
    ArrayAppend,
    ArrayInsert,
    ArraySet,
    ArrayReplace,
    ArrayExchange,
    ArrayRemove,
    ArrayRemoveAll,
    DictionaryCreate(bool),
    DictionaryCopy,
    DictionaryCount,
    DictionaryValue,
    DictionaryContainsKey,
    DictionaryContainsValue,
    DictionarySet,
    DictionaryRemove,
    DictionaryRemoveAll,
    DictionaryKeysAndValues,
    StringCreate(bool),
    StringCreateCharacters,
    StringCreateBytes,
    StringLength,
    StringCharacter,
    StringGetCString,
    StringCStringPointer,
    StringMaximumSize,
    StringAppend,
    StringAppendCString,
    DataCreate(bool),
    DataCreateCopy,
    DataBytePointer,
    DataGetBytes,
    DataLength,
    DataAppend,
    NumberCreate,
    NumberValue,
    Retain,
    Release,
    Allocator,
    Null,
    GenericPointer,
    GenericReceiver,
    GenericZero,
    ExceptionWhat,
    ExceptionConstructor,
    ExceptionPointer,
    CryptoNoop,
    CMTimeGetSeconds,
    CMTimeMakeWithSeconds,
    CVTextureCacheCreate,
    CVTextureName,
    CVTextureTarget,
    DispatchDataApply,
    Asprintf,
    GetProgname,
    DigitToInt,
    IsXDigit,
    StringCompare,
    VmMap,
    VmReadOverwrite,
    StdioFilePointer,
    UnwindResume,
    GxxPersonality,
    OperatorDeleteNothrow,
    ZlibVersion,
    Crc32,
    ZlibUncompress,
    ZlibInit,
    GzOpen,
    GzRead,
    GzClose,
    SemTimedwait,
    SetIOPolicy,
    Dup2,
    Execl,
    Socketpair,
    TcGetSetAttr,
    MethodGetNumberOfArguments,
    ObjectSetClass,
    Basename,
    AUGraphGetNodeCount,
    AudioQueueDeviceGetCurrentTime,
    AudioQueueOfflineRender,
    CMSampleBufferCreate,
    CMSampleBufferCreateCopyWithNewTiming,
    CMSampleBufferGetFormatDescription,
    CMSampleBufferGetPresentationTimeStamp,
    CMSampleBufferGetSampleTimingInfo,
    CMSampleBufferSetDataBufferFromAudioBufferList,
    CMSampleBufferSetDataReady,
    CMSampleBufferGetDataIsReady,
    CMAudioFormatDescriptionCreate,
    CMAudioFormatDescriptionGetStreamBasicDescription,
    CMAudioFormatDescriptionGetChannelLayout,
    CMTimeAdd,
    CMTimeSubtract,
    CMTimeCompare,
    CMTimeConvertScale,
    CMTimeCopyDescription,
    CMTimeRangeMake,
    CMTimeRangeContainsTime,
    CVMetalTextureGetTexture,
}

/// CRC-32 (IEEE 802.3, the table zlib uses for `crc32()`).
static CRC32_TABLE: LazyLock<[u32; 256]> = LazyLock::new(|| {
    let mut table = [0u32; 256];
    for (index, entry) in table.iter_mut().enumerate() {
        let mut value = index as u32;
        for _ in 0..8 {
            value = if value & 1 != 0 {
                0xedb8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
        }
        *entry = value;
    }
    table
});

/// Monotonically increasing fake sample clock for
/// `AudioQueueDeviceGetCurrentTime` (units of 1/1024 sample @ 1024 Hz).
static GUEST_SAMPLE_CLOCK: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Default)]
struct GuestCmTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

fn read_cm_time(mem: &Mem64, pointer: u64) -> GuestCmTime {
    if pointer == 0 || mem.allocation_size(pointer).is_none() {
        return GuestCmTime::default();
    }
    GuestCmTime {
        value: mem.read_u64(pointer).unwrap_or(0) as i64,
        timescale: mem.read_u32(pointer.saturating_add(8)).unwrap_or(0) as i32,
        flags: mem.read_u32(pointer.saturating_add(12)).unwrap_or(0),
        epoch: mem.read_u64(pointer.saturating_add(16)).unwrap_or(0) as i64,
    }
}

fn write_cm_time_at(
    mem: &mut Mem64,
    pointer: u64,
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
) -> Result<(), String> {
    mem.write_u64(pointer, value as u64)
        .map_err(str::to_owned)?;
    mem.write_u32(pointer.saturating_add(8), timescale as u32)
        .map_err(str::to_owned)?;
    mem.write_u32(pointer.saturating_add(12), flags)
        .map_err(str::to_owned)?;
    mem.write_u64(pointer.saturating_add(16), epoch as u64)
        .map_err(str::to_owned)
}

/// CMTime struct returns come back through the x8 indirect-result slot.
fn write_cm_time(
    context: &mut touchHLE_DynarmicA64Context,
    mem: &mut Mem64,
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
) -> Result<(), String> {
    let out = context.regs[8];
    if out != 0 && mem.allocation_size(out).is_some() {
        write_cm_time_at(mem, out, value, timescale, flags, epoch)?;
    }
    super::return_value(context, out);
    Ok(())
}

fn cm_time_seconds(time: &GuestCmTime) -> f64 {
    if time.timescale == 0 {
        return 0.0;
    }
    time.value as f64 / time.timescale as f64
}

fn lcm_timescale(a: i32, b: i32) -> i64 {
    let (a, b) = (i64::from(a.max(1)), i64::from(b.max(1)));
    let gcd = {
        let (mut x, mut y) = (a, b);
        while y != 0 {
            (x, y) = (y, x % y);
        }
        x
    };
    a / gcd * b
}

/// Best-effort stack argument reader: AArch64 AAPCS passes the 9th+ argument
/// on the stack above SP. The dynarmic context does not expose SP, so this
/// reads the caller-provided shadow register slot if the address is mapped.
fn stack_arg(context: &touchHLE_DynarmicA64Context, index: u64) -> Option<u64> {
    let slot = context.regs.get((9 + index) as usize).copied()?;
    if slot == 0 {
        None
    } else {
        Some(slot)
    }
}

/// Counts the arguments in an Objective-C type encoding string (excluding the
/// return type, including self/_cmd), mirroring
/// `method_getNumberOfArguments`.
fn objc_encoding_argument_count(encoding: &[u8]) -> u32 {
    let mut count = 0u32;
    let mut index = 0usize;
    // Skip the return type (with its qualifiers and nested braces).
    index += skip_type_encoding(&encoding[index..]);
    while index < encoding.len() {
        let byte = encoding[index];
        if byte == 0 {
            break;
        }
        index += skip_type_encoding(&encoding[index..]);
        count += 1;
    }
    count + 2
}

fn skip_type_encoding(encoding: &[u8]) -> usize {
    let mut offset = 0usize;
    while offset < encoding.len() {
        match encoding[offset] {
            b'r' | b'n' | b'N' | b'o' | b'O' | b'R' | b'V' => offset += 1,
            _ => break,
        }
    }
    if offset >= encoding.len() {
        return offset;
    }
    let open = encoding[offset];
    let close = match open {
        b'{' => b'}',
        b'(' => b')',
        b'[' => b']',
        b'^' => return offset + 1 + skip_type_encoding(&encoding[offset + 1..]),
        b'b' => {
            // Bit-field: b<start><length><type>
            return offset + 1 + skip_type_encoding(&encoding[offset + 1..]);
        }
        _ => return offset + 1,
    };
    let mut depth = 0usize;
    while offset < encoding.len() {
        if encoding[offset] == open {
            depth += 1;
        } else if encoding[offset] == close {
            depth -= 1;
            if depth == 0 {
                return offset + 1;
            }
        }
        offset += 1;
    }
    offset
}

fn copy_sample_format_description(mem: &Mem64, sample: u64) -> u64 {
    mem.read_u64(sample.saturating_add(32)).unwrap_or(0)
}

fn normalized(symbol: &str) -> &str {
    let symbol = symbol.trim_start_matches('_');
    symbol.strip_prefix('_').unwrap_or(symbol)
}

fn compatibility_kind(symbol: &str) -> Option<StubKind> {
    match symbol {
        "CCHmacInit" | "CCHmacUpdate" | "CCHmacFinal" => Some(StubKind::CryptoNoop),
        "Unwind_Resume" => Some(StubKind::UnwindResume),
        "gxx_personality_v0" => Some(StubKind::GxxPersonality),
        "ZdlPvRKSt9nothrow_t" => Some(StubKind::OperatorDeleteNothrow),
        "zlibVersion" => Some(StubKind::ZlibVersion),
        "crc32" => Some(StubKind::Crc32),
        "uncompress" => Some(StubKind::ZlibUncompress),
        "deflateInit_" | "deflateInit2_" | "inflateInit_" | "inflateInit2_" => {
            Some(StubKind::ZlibInit)
        }
        "gzopen" | "gzdopen" => Some(StubKind::GzOpen),
        "gzread" | "gzwrite" => Some(StubKind::GzRead),
        "gzclose" => Some(StubKind::GzClose),
        "semaphore_timedwait" => Some(StubKind::SemTimedwait),
        "setiopolicy_np" => Some(StubKind::SetIOPolicy),
        "dup2" => Some(StubKind::Dup2),
        "execl" => Some(StubKind::Execl),
        "socketpair" => Some(StubKind::Socketpair),
        "tcgetattr" | "tcsetattr" => Some(StubKind::TcGetSetAttr),
        "method_getNumberOfArguments" => Some(StubKind::MethodGetNumberOfArguments),
        "object_setClass" => Some(StubKind::ObjectSetClass),
        "basename" => Some(StubKind::Basename),
        "AUGraphGetNodeCount" => Some(StubKind::AUGraphGetNodeCount),
        "AudioQueueDeviceGetCurrentTime" | "AudioQueueDeviceGetNearestStartTime" => {
            Some(StubKind::AudioQueueDeviceGetCurrentTime)
        }
        "AudioQueueOfflineRender" => Some(StubKind::AudioQueueOfflineRender),
        "CMSampleBufferCreate" => Some(StubKind::CMSampleBufferCreate),
        "CMSampleBufferCreateCopyWithNewTiming" => {
            Some(StubKind::CMSampleBufferCreateCopyWithNewTiming)
        }
        "CMSampleBufferGetFormatDescription" => Some(StubKind::CMSampleBufferGetFormatDescription),
        "CMSampleBufferGetPresentationTimeStamp" => {
            Some(StubKind::CMSampleBufferGetPresentationTimeStamp)
        }
        "CMSampleBufferGetSampleTimingInfo" => Some(StubKind::CMSampleBufferGetSampleTimingInfo),
        "CMSampleBufferSetDataBufferFromAudioBufferList" => {
            Some(StubKind::CMSampleBufferSetDataBufferFromAudioBufferList)
        }
        "CMSampleBufferSetDataReady" => Some(StubKind::CMSampleBufferSetDataReady),
        "CMSampleBufferDataIsReady" => Some(StubKind::CMSampleBufferGetDataIsReady),
        "CMAudioFormatDescriptionCreate" => Some(StubKind::CMAudioFormatDescriptionCreate),
        "CMAudioFormatDescriptionGetStreamBasicDescription" => {
            Some(StubKind::CMAudioFormatDescriptionGetStreamBasicDescription)
        }
        "CMAudioFormatDescriptionGetChannelLayout" => {
            Some(StubKind::CMAudioFormatDescriptionGetChannelLayout)
        }
        "CMTimeAdd" => Some(StubKind::CMTimeAdd),
        "CMTimeSubtract" => Some(StubKind::CMTimeSubtract),
        "CMTimeCompare" => Some(StubKind::CMTimeCompare),
        "CMTimeConvertScale" => Some(StubKind::CMTimeConvertScale),
        "CMTimeCopyDescription" => Some(StubKind::CMTimeCopyDescription),
        "CMTimeRangeMake" => Some(StubKind::CMTimeRangeMake),
        "CMTimeRangeContainsTime" => Some(StubKind::CMTimeRangeContainsTime),
        "CVMetalTextureGetTexture" => Some(StubKind::CVMetalTextureGetTexture),
        "CMTimeGetSeconds" => Some(StubKind::CMTimeGetSeconds),
        "CMTimeMakeWithSeconds" => Some(StubKind::CMTimeMakeWithSeconds),
        "CVOpenGLESTextureCacheCreate"
        | "CVOpenGLESTextureCacheCreateTextureFromImage"
        | "CVOpenGLESTextureCacheFlush" => Some(StubKind::CVTextureCacheCreate),
        "CVOpenGLESTextureGetName" => Some(StubKind::CVTextureName),
        "CVOpenGLESTextureGetTarget" => Some(StubKind::CVTextureTarget),
        "dispatch_data_apply" => Some(StubKind::DispatchDataApply),
        "dispatch_data_create"
        | "UTTypeCopyPreferredTagWithClass"
        | "UTTypeCreatePreferredIdentifierForTag" => Some(StubKind::GenericPointer),
        "dispatch_data_get_size" | "dispatch_read" | "dispatch_write" => {
            Some(StubKind::GenericZero)
        }
        "asprintf" => Some(StubKind::Asprintf),
        "getprogname" => Some(StubKind::GetProgname),
        "digittoint" => Some(StubKind::DigitToInt),
        "isxdigit" => Some(StubKind::IsXDigit),
        "strcoll" => Some(StubKind::StringCompare),
        "vm_map" => Some(StubKind::VmMap),
        "vm_read_overwrite" => Some(StubKind::VmReadOverwrite),
        "regcomp"
        | "regexec"
        | "readdir_r"
        | "nftw"
        | "utime"
        | "pathconf"
        | "arc4random_buf"
        | "class_conformsToProtocol"
        | "protocol_getMethodDescription"
        | "objc_exception_rethrow"
        | "objc_terminate"
        | "exception_raise"
        | "exception_raise_state"
        | "exception_raise_state_identity"
        | "mach_make_memory_entry_64"
        | "mach_port_mod_refs"
        | "mach_port_move_member"
        | "mach_port_request_notification"
        | "thread_get_exception_ports"
        | "thread_swap_exception_ports"
        | "kill"
        | "raise"
        | "DNSServiceNATPortMappingCreate"
        | "DNSServiceProcessResult"
        | "DNSServiceRefDeallocate"
        | "___objc_personality_v0"
        | "objc_personality_v0"
        | "cxa_bad_cast"
        | "ZSt17rethrow_exceptionSt13exception_ptr"
        | "ZSt18uncaught_exceptionv" => Some(StubKind::GenericZero),
        "cxa_get_exception_ptr" => Some(StubKind::ExceptionPointer),
        "ZSt17current_exceptionv" => Some(StubKind::GenericPointer),
        "_hash_create" | "hash_create" | "_hash_search" | "hash_search" | "getpwnam" => {
            Some(StubKind::GenericPointer)
        }
        symbol
            if symbol.starts_with("ZNKSt9exception4what")
                || symbol.starts_with("ZNKSt13runtime_error4what") =>
        {
            Some(StubKind::ExceptionWhat)
        }
        symbol
            if symbol.starts_with("ZNSt11logic_errorC")
                || symbol.starts_with("ZNSt13runtime_errorC") =>
        {
            Some(StubKind::ExceptionConstructor)
        }
        symbol if symbol.starts_with("ZN7plcrash") => {
            if symbol.contains("C1") || symbol.contains("C2") {
                Some(StubKind::GenericReceiver)
            } else {
                Some(StubKind::GenericZero)
            }
        }
        symbol if symbol.starts_with("ZThn") || symbol.starts_with("ZTv") => {
            Some(StubKind::GenericReceiver)
        }
        symbol
            if symbol.starts_with("ZNSt")
                || symbol.starts_with("ZNKSt")
                || symbol.starts_with("ZSt") =>
        {
            if symbol.contains("D1") || symbol.contains("D2") {
                Some(StubKind::GenericReceiver)
            } else if symbol.contains("what") {
                Some(StubKind::ExceptionWhat)
            } else if symbol.contains("C1") || symbol.contains("C2") {
                Some(StubKind::ExceptionConstructor)
            } else {
                Some(StubKind::GenericReceiver)
            }
        }
        _ => None,
    }
}

fn generic_kind(symbol: &str) -> Option<StubKind> {
    let symbol = normalized(symbol);
    if let Some(kind) = compatibility_kind(symbol) {
        return Some(kind);
    }
    if symbol.starts_with("CFArray") {
        return Some(match symbol {
            "CFArrayCreate" => StubKind::ArrayCreate(false),
            "CFArrayCreateMutable" => StubKind::ArrayCreate(true),
            "CFArrayGetCount" => StubKind::ArrayCount,
            "CFArrayGetValueAtIndex" => StubKind::ArrayValue,
            "CFArrayGetValues" => StubKind::ArrayGetValues,
            "CFArrayGetFirstIndexOfValue" => StubKind::ArrayFirstIndex,
            "CFArrayContainsValue" => StubKind::ArrayContains,
            "CFArrayAppendValue" => StubKind::ArrayAppend,
            "CFArrayInsertValueAtIndex" => StubKind::ArrayInsert,
            "CFArraySetValueAtIndex" => StubKind::ArraySet,
            "CFArrayReplaceValues" => StubKind::ArrayReplace,
            "CFArrayExchangeValuesAtIndices" => StubKind::ArrayExchange,
            "CFArrayRemoveValueAtIndex" => StubKind::ArrayRemove,
            "CFArrayRemoveAllValues" => StubKind::ArrayRemoveAll,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFDictionary") {
        return Some(match symbol {
            "CFDictionaryCreate" => StubKind::DictionaryCreate(false),
            "CFDictionaryCreateMutable" => StubKind::DictionaryCreate(true),
            "CFDictionaryCreateCopy" => StubKind::DictionaryCopy,
            "CFDictionaryGetCount" => StubKind::DictionaryCount,
            "CFDictionaryGetValue" => StubKind::DictionaryValue,
            "CFDictionaryContainsKey" => StubKind::DictionaryContainsKey,
            "CFDictionaryContainsValue" => StubKind::DictionaryContainsValue,
            "CFDictionarySetValue" => StubKind::DictionarySet,
            "CFDictionaryRemoveValue" => StubKind::DictionaryRemove,
            "CFDictionaryRemoveAllValues" => StubKind::DictionaryRemoveAll,
            "CFDictionaryGetKeysAndValues" => StubKind::DictionaryKeysAndValues,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFString") {
        return Some(match symbol {
            "CFStringCreate" => StubKind::StringCreate(true),
            "CFStringCreateWithCString" => StubKind::StringCreate(false),
            "CFStringCreateWithCharacters" => StubKind::StringCreateCharacters,
            "CFStringCreateWithBytes" => StubKind::StringCreateBytes,
            "CFStringCreateMutable" => StubKind::StringCreate(false),
            "CFStringGetLength" => StubKind::StringLength,
            "CFStringGetCharacterAtIndex" => StubKind::StringCharacter,
            "CFStringGetCString" => StubKind::StringGetCString,
            "CFStringGetCStringPtr" => StubKind::StringCStringPointer,
            "CFStringGetMaximumSizeForEncoding" => StubKind::StringMaximumSize,
            "CFStringAppend" => StubKind::StringAppend,
            "CFStringAppendCString" => StubKind::StringAppendCString,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFData") {
        return Some(match symbol {
            "CFDataCreate" => StubKind::DataCreate(false),
            "CFDataCreateMutable" => StubKind::DataCreate(true),
            "CFDataCreateCopy" => StubKind::DataCreateCopy,
            "CFDataGetBytePtr" | "CFDataGetMutableBytePtr" => StubKind::DataBytePointer,
            "CFDataGetBytes" => StubKind::DataGetBytes,
            "CFDataGetLength" => StubKind::DataLength,
            "CFDataAppendBytes" => StubKind::DataAppend,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFNumber") {
        return Some(match symbol {
            "CFNumberCreate" => StubKind::NumberCreate,
            "CFNumberGetValue" => StubKind::NumberValue,
            _ => StubKind::GenericPointer,
        });
    }
    match symbol {
        "CFNull" | "kCFNull" => Some(StubKind::Null),
        "CFGetRetainCount" => Some(StubKind::NumberValue),
        "CFRetain" | "CFAutorelease" => Some(StubKind::Retain),
        "CFRelease" => Some(StubKind::Release),
        "CFAllocatorCreate" | "CFAllocatorGetDefault" | "kCFAllocatorDefault" => {
            Some(StubKind::Allocator)
        }
        "NSArrayObjectAtIndex" => Some(StubKind::ArrayValue),
        "NSArrayCount" => Some(StubKind::ArrayCount),
        "NSDictionaryObjectForKey" => Some(StubKind::DictionaryValue),
        "NSStringFromClass" | "NSClassFromString" | "NSStringFromSelector" => {
            Some(StubKind::GenericPointer)
        }
        _ if symbol.starts_with("CF")
            || symbol.starts_with("CG")
            || symbol.starts_with("NS")
            || symbol.starts_with("UI") =>
        {
            if symbol.contains("Create")
                || symbol.contains("Copy")
                || symbol.contains("Alloc")
                || symbol.contains("New")
                || symbol.contains("Class")
                || symbol.contains("FromString")
                || symbol.contains("With")
            {
                Some(StubKind::GenericPointer)
            } else if symbol.contains("Init") {
                Some(StubKind::GenericReceiver)
            } else if symbol.contains("Set")
                || symbol.contains("Add")
                || symbol.contains("Append")
                || symbol.contains("Remove")
                || symbol.contains("Release")
                || symbol.contains("Destroy")
            {
                Some(StubKind::GenericReceiver)
            } else if symbol.contains("Count")
                || symbol.contains("Length")
                || symbol.contains("Value")
                || symbol.contains("TypeID")
                || symbol.contains("Index")
                || symbol.contains("Size")
                || symbol.contains("Width")
                || symbol.contains("Height")
                || symbol.contains("Is")
                || symbol.contains("Has")
                || symbol.contains("Equal")
                || symbol.contains("Compare")
                || symbol.contains("Status")
                || symbol.contains("Error")
            {
                Some(StubKind::GenericZero)
            } else {
                Some(StubKind::GenericPointer)
            }
        }
        _ => None,
    }
}

pub(super) fn is_known(symbol: &str) -> bool {
    generic_kind(symbol).is_some()
}

fn array_values(mem: &Mem64, array: u64) -> Vec<u64> {
    let count = objc_field(mem, array, 56).min(4096);
    let elements = objc_field(mem, array, 64);
    (0..count)
        .filter_map(|index| mem.read_u64(elements.saturating_add(index * 8)).ok())
        .collect()
}
fn range_values(mem: &Mem64, range: u64, available: usize) -> (usize, usize) {
    if range == 0 {
        return (0, 0);
    }
    let location = mem.read_u64(range).unwrap_or(0).min(available as u64) as usize;
    let length = mem
        .read_u64(range.saturating_add(8))
        .unwrap_or(0)
        .min((available - location) as u64) as usize;
    (location, length)
}

fn guest_values(mem: &Mem64, pointer: u64, count: u64) -> Vec<u64> {
    if pointer == 0 {
        return Vec::new();
    }
    (0..count.min(4096))
        .filter_map(|index| mem.read_u64(pointer.saturating_add(index * 8)).ok())
        .collect()
}

fn write_array_values(mem: &mut Mem64, array: u64, values: &[u64]) -> Result<(), String> {
    let elements = mem
        .alloc_zeroed((values.len().max(1) as u64).saturating_mul(8))
        .map_err(str::to_owned)?;
    for (index, value) in values.iter().copied().enumerate() {
        mem.write_u64(elements + index as u64 * 8, value)
            .map_err(str::to_owned)?;
    }
    set_objc_field(mem, array, 56, values.len() as u64);
    set_objc_field(mem, array, 64, elements);
    Ok(())
}

fn dictionary_pairs(mem: &Mem64, dictionary: u64) -> (Vec<u64>, Vec<u64>) {
    let count = objc_field(mem, dictionary, 56).min(4096);
    let keys = objc_field(mem, dictionary, 64);
    let values = objc_field(mem, dictionary, 72);
    let keys = (0..count)
        .filter_map(|index| mem.read_u64(keys.saturating_add(index * 8)).ok())
        .collect();
    let values = (0..count)
        .filter_map(|index| mem.read_u64(values.saturating_add(index * 8)).ok())
        .collect();
    (keys, values)
}

fn write_dictionary_pairs(
    mem: &mut Mem64,
    dictionary: u64,
    keys: &[u64],
    values: &[u64],
) -> Result<(), String> {
    let count = keys.len().min(values.len());
    let key_storage = mem
        .alloc_zeroed((count.max(1) as u64).saturating_mul(8))
        .map_err(str::to_owned)?;
    let value_storage = mem
        .alloc_zeroed((count.max(1) as u64).saturating_mul(8))
        .map_err(str::to_owned)?;
    for index in 0..count {
        mem.write_u64(key_storage + index as u64 * 8, keys[index])
            .map_err(str::to_owned)?;
        mem.write_u64(value_storage + index as u64 * 8, values[index])
            .map_err(str::to_owned)?;
    }
    set_objc_field(mem, dictionary, 56, count as u64);
    set_objc_field(mem, dictionary, 64, key_storage);
    set_objc_field(mem, dictionary, 72, value_storage);
    Ok(())
}

fn dictionary_create(
    mem: &mut Mem64,
    keys_pointer: u64,
    values_pointer: u64,
    count: u64,
    mutable: bool,
) -> Result<u64, String> {
    let count = count.min(4096);
    let keys = if keys_pointer == 0 {
        Vec::new()
    } else {
        (0..count)
            .filter_map(|index| mem.read_u64(keys_pointer + index * 8).ok())
            .collect::<Vec<_>>()
    };
    let values = if values_pointer == 0 {
        Vec::new()
    } else {
        (0..count)
            .filter_map(|index| mem.read_u64(values_pointer + index * 8).ok())
            .collect::<Vec<_>>()
    };
    let object = objc_object(
        mem,
        if mutable {
            A64_KIND_MUTABLE_DICTIONARY
        } else {
            A64_KIND_DICTIONARY
        },
    )?;
    write_dictionary_pairs(mem, object, &keys, &values)?;
    Ok(object)
}

fn string_from_utf16(mem: &Mem64, pointer: u64, count: u64) -> String {
    let mut value = String::new();
    for index in 0..count.min(1_048_576) {
        let Ok(character) = mem.read_u16(pointer.saturating_add(index * 2)) else {
            break;
        };
        value.push(char::from_u32(u32::from(character)).unwrap_or('\u{fffd}'));
    }
    value
}

fn replace_string(mem: &mut Mem64, object: u64, bytes: &[u8]) -> Result<(), String> {
    let pointer = mem
        .alloc_zeroed(bytes.len() as u64 + 1)
        .map_err(str::to_owned)?;
    if !bytes.is_empty() {
        mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    }
    mem.write_u8(pointer + bytes.len() as u64, 0)
        .map_err(str::to_owned)?;
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(())
}

fn replace_data(mem: &mut Mem64, object: u64, bytes: &[u8]) -> Result<(), String> {
    let pointer = mem
        .alloc_zeroed(bytes.len().max(1) as u64)
        .map_err(str::to_owned)?;
    if !bytes.is_empty() {
        mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    }
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(())
}

fn read_number_as_i64(mem: &Mem64, pointer: u64, number_type: u64) -> i64 {
    match number_type {
        1 | 7 => mem
            .read_u8(pointer)
            .map(|value| value as i8)
            .unwrap_or_default() as i64,
        2 | 8 => mem
            .read_u16(pointer)
            .map(|value| i16::from_le_bytes(value.to_le_bytes()) as i64)
            .unwrap_or_default(),
        3 | 9 => mem
            .read_u32(pointer)
            .map(|value| i32::from_le_bytes(value.to_le_bytes()) as i64)
            .unwrap_or_default(),
        4 | 10 | 11 | 14 | 15 => mem
            .read_u64(pointer)
            .map(|value| i64::from_le_bytes(value.to_le_bytes()))
            .unwrap_or_default(),
        _ => mem
            .read_u64(pointer)
            .map(|value| i64::from_le_bytes(value.to_le_bytes()))
            .unwrap_or_default(),
    }
}

fn read_number_as_f64(mem: &Mem64, pointer: u64, number_type: u64) -> f64 {
    match number_type {
        5 | 12 => mem
            .read_u32(pointer)
            .map(f32::from_bits)
            .unwrap_or_default() as f64,
        6 | 13 | 16 => mem
            .read_u64(pointer)
            .map(f64::from_bits)
            .unwrap_or_default(),
        _ => read_number_as_i64(mem, pointer, number_type) as f64,
    }
}

fn write_number_value(
    mem: &mut Mem64,
    pointer: u64,
    number_type: u64,
    value: f64,
) -> Result<(), String> {
    let result = match number_type {
        1 | 7 => mem.write_u8(pointer, value as i8 as u8),
        2 | 8 => mem.write_u16(pointer, (value as i16) as u16),
        3 | 9 => mem.write_u32(pointer, (value as i32) as u32),
        4 | 10 | 11 | 14 | 15 => mem.write_u64(pointer, (value as i64) as u64),
        5 | 12 => mem.write_u32(pointer, (value as f32).to_bits()),
        6 | 13 | 16 => mem.write_u64(pointer, value.to_bits()),
        _ => mem.write_u64(pointer, (value as i64) as u64),
    };
    result.map_err(str::to_owned)
}

fn generic_pointer(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
) -> Result<(), String> {
    let receiver = context.regs[0];
    let value = if receiver != 0 && mem.allocation_size(receiver).is_some() {
        receiver
    } else {
        objc_object(mem, A64_KIND_GENERIC)?
    };
    super::return_value(context, value);
    Ok(())
}

pub(super) fn dispatch(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    symbol: &str,
) -> Result<bool, String> {
    let Some(kind) = generic_kind(symbol) else {
        return Ok(false);
    };
    let symbol = normalized(symbol);
    log_once_fmt!(
        "ARM64 unresolved import implementation selected: {} [repeated calls suppressed]",
        symbol
    );
    match kind {
        StubKind::ExceptionWhat => {
            let receiver = context.regs[0];
            let message = if receiver != 0 {
                let pointer = objc_field(mem, receiver, 56);
                c_string(mem, pointer).filter(|bytes| !bytes.is_empty())
            } else {
                None
            };
            let message = message.unwrap_or_else(|| b"std::exception".to_vec());
            let pointer = mem
                .alloc_zeroed(message.len() as u64 + 1)
                .map_err(str::to_owned)?;
            mem.write_bytes(pointer, &message).map_err(str::to_owned)?;
            mem.write_u8(pointer + message.len() as u64, 0)
                .map_err(str::to_owned)?;
            super::return_value(context, pointer);
        }
        StubKind::ExceptionConstructor => {
            if context.regs[0] != 0 && mem.allocation_size(context.regs[0]).is_some() {
                if context.regs[1] != 0 && mem.allocation_size(context.regs[1]).is_some() {
                    let pointer = context.regs[1];
                    set_objc_field(mem, context.regs[0], 56, pointer);
                }
                super::return_value(context, context.regs[0]);
            } else {
                super::return_value(context, 0);
            }
        }
        StubKind::ExceptionPointer => super::return_value(context, context.regs[0]),
        StubKind::CryptoNoop => {
            if context.regs[0] != 0 && context.regs[1] != 0 {
                let _ = mem.write_bytes(context.regs[1], &vec![0u8; 64]);
            }
            super::return_value(context, 0);
        }
        StubKind::CMTimeGetSeconds => {
            let value = if context.regs[0] != 0 && mem.allocation_size(context.regs[0]).is_some() {
                let numerator = mem.read_u64(context.regs[0]).unwrap_or(0) as i64;
                let scale = mem.read_u32(context.regs[0] + 8).unwrap_or(0) as i32;
                if scale == 0 {
                    0.0
                } else {
                    numerator as f64 / scale as f64
                }
            } else {
                0.0
            };
            context.vectors[0][0] = value.to_bits();
            super::return_value(context, value.to_bits());
        }
        StubKind::CMTimeMakeWithSeconds => {
            let seconds = f64::from_bits(context.vectors[0][0]);
            let timescale = context.regs[0] as i32;
            let output = context.regs[8];
            if output != 0 && mem.allocation_size(output).is_some() {
                mem.write_u64(output, (seconds * timescale as f64).round() as i64 as u64)
                    .map_err(str::to_owned)?;
                mem.write_u32(output + 8, timescale as u32)
                    .map_err(str::to_owned)?;
                mem.write_u32(output + 12, 1).map_err(str::to_owned)?;
                mem.write_u64(output + 16, 0).map_err(str::to_owned)?;
                super::return_value(context, output);
            } else {
                super::return_value(context, 0);
            }
        }
        StubKind::CVTextureCacheCreate => {
            if symbol == "CVOpenGLESTextureCacheCreate" && context.regs[4] != 0 {
                let object = objc_object(mem, A64_KIND_GENERIC)?;
                if mem.allocation_size(context.regs[4]).is_some() {
                    mem.write_u64(context.regs[4], object)
                        .map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::CVTextureName => {
            super::return_value(context, objc_field(mem, context.regs[0], 56) as u32 as u64);
        }
        StubKind::CVTextureTarget => super::return_value(context, 0x0de1),
        StubKind::DispatchDataApply => super::return_value(context, 1),
        StubKind::Asprintf => {
            let output = context.regs[0];
            let format = c_string(mem, context.regs[1]).unwrap_or_default();
            let pointer = mem
                .alloc_zeroed(format.len() as u64 + 1)
                .map_err(str::to_owned)?;
            mem.write_bytes(pointer, &format).map_err(str::to_owned)?;
            mem.write_u8(pointer + format.len() as u64, 0)
                .map_err(str::to_owned)?;
            if output != 0 && mem.allocation_size(output).is_some() {
                mem.write_u64(output, pointer).map_err(str::to_owned)?;
            }
            super::return_value(context, format.len() as u64);
        }
        StubKind::GetProgname => {
            let value = b"MetalHLE";
            let pointer = mem
                .alloc_zeroed(value.len() as u64 + 1)
                .map_err(str::to_owned)?;
            mem.write_bytes(pointer, value).map_err(str::to_owned)?;
            super::return_value(context, pointer);
        }
        StubKind::DigitToInt => {
            let value = (context.regs[0] as u8 as char).to_digit(16).unwrap_or(0);
            super::return_value(context, value as u64);
        }
        StubKind::IsXDigit => {
            super::return_value(
                context,
                u64::from((context.regs[0] as u8 as char).is_ascii_hexdigit()),
            );
        }
        StubKind::StringCompare => {
            let left = c_string(mem, context.regs[0]).unwrap_or_default();
            let right = c_string(mem, context.regs[1]).unwrap_or_default();
            let value = match left.cmp(&right) {
                std::cmp::Ordering::Less => -1_i64,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            super::return_value(context, value as u64);
        }
        StubKind::VmMap => {
            let size = context.regs[2].min(64 * 1024 * 1024).max(1);
            let address = mem.alloc_zeroed(size).map_err(str::to_owned)?;
            if context.regs[1] != 0 && mem.allocation_size(context.regs[1]).is_some() {
                mem.write_u64(context.regs[1], address)
                    .map_err(str::to_owned)?;
            }
            super::return_value(context, 0);
        }
        StubKind::VmReadOverwrite => super::return_value(context, 0),
        StubKind::ArrayCreate(mutable) => {
            let (values, count) = if mutable {
                (0, 0)
            } else {
                (context.regs[1], context.regs[2].min(4096))
            };
            let objects = if values == 0 {
                Vec::new()
            } else {
                (0..count)
                    .filter_map(|index| mem.read_u64(values + index * 8).ok())
                    .collect::<Vec<_>>()
            };
            let object = super::objc_array_with_kind(
                mem,
                if mutable {
                    A64_KIND_MUTABLE_ARRAY
                } else {
                    A64_KIND_ARRAY
                },
                &objects,
            )?;
            super::return_value(context, object);
        }
        StubKind::ArrayCount => super::return_value(context, objc_field(mem, context.regs[0], 56)),
        StubKind::ArrayValue => {
            let values = array_values(mem, context.regs[0]);
            super::return_value(
                context,
                values.get(context.regs[1] as usize).copied().unwrap_or(0),
            );
        }
        StubKind::ArrayGetValues => {
            let values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            if context.regs[2] != 0 {
                for (index, value) in values[location..location + length]
                    .iter()
                    .copied()
                    .enumerate()
                {
                    mem.write_u64(context.regs[2] + index as u64 * 8, value)
                        .map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayFirstIndex => {
            let values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            let index = values[location..location + length]
                .iter()
                .position(|value| *value == context.regs[2])
                .map(|index| location + index)
                .map(|index| index as u64)
                .unwrap_or(u64::MAX);
            super::return_value(context, index);
        }
        StubKind::ArrayContains => {
            let values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            super::return_value(
                context,
                u64::from(
                    values[location..location + length]
                        .iter()
                        .any(|value| *value == context.regs[2]),
                ),
            );
        }
        StubKind::ArrayAppend => {
            let mut values = array_values(mem, context.regs[0]);
            values.push(context.regs[1]);
            write_array_values(mem, context.regs[0], &values)?;
            super::return_value(context, 0);
        }
        StubKind::ArrayInsert => {
            let mut values = array_values(mem, context.regs[0]);
            let index = (context.regs[1] as usize).min(values.len());
            values.insert(index, context.regs[2]);
            write_array_values(mem, context.regs[0], &values)?;
            super::return_value(context, 0);
        }
        StubKind::ArraySet => {
            let mut values = array_values(mem, context.regs[0]);
            let index = context.regs[1] as usize;
            if index < values.len() {
                values[index] = context.regs[2];
                write_array_values(mem, context.regs[0], &values)?;
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayReplace => {
            let mut values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            let replacement = guest_values(mem, context.regs[2], context.regs[3]);
            values.splice(location..location + length, replacement);
            write_array_values(mem, context.regs[0], &values)?;
            super::return_value(context, 0);
        }
        StubKind::ArrayExchange => {
            let mut values = array_values(mem, context.regs[0]);
            let first = context.regs[1] as usize;
            let second = context.regs[2] as usize;
            if first < values.len() && second < values.len() {
                values.swap(first, second);
                write_array_values(mem, context.regs[0], &values)?;
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayRemove => {
            let mut values = array_values(mem, context.regs[0]);
            let index = context.regs[1] as usize;
            if index < values.len() {
                values.remove(index);
                write_array_values(mem, context.regs[0], &values)?;
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayRemoveAll => {
            write_array_values(mem, context.regs[0], &[])?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryCreate(mutable) => {
            let (keys, values, count) = if mutable {
                (0, 0, 0)
            } else {
                (context.regs[1], context.regs[2], context.regs[3].min(4096))
            };
            let object = dictionary_create(mem, keys, values, count, mutable)?;
            super::return_value(context, object);
        }
        StubKind::DictionaryCopy => {
            let (keys, values) = dictionary_pairs(mem, context.regs[1]);
            let object = objc_object(mem, A64_KIND_DICTIONARY)?;
            write_dictionary_pairs(mem, object, &keys, &values)?;
            super::return_value(context, object);
        }
        StubKind::DictionaryCount => {
            super::return_value(context, objc_field(mem, context.regs[0], 56));
        }
        StubKind::DictionaryValue => {
            let (keys, values) = dictionary_pairs(mem, context.regs[0]);
            let value = keys
                .iter()
                .position(|key| *key == context.regs[1])
                .and_then(|index| values.get(index).copied())
                .unwrap_or(0);
            super::return_value(context, value);
        }
        StubKind::DictionaryContainsKey => {
            let (keys, _) = dictionary_pairs(mem, context.regs[0]);
            super::return_value(context, u64::from(keys.contains(&context.regs[1])));
        }
        StubKind::DictionaryContainsValue => {
            let (_, values) = dictionary_pairs(mem, context.regs[0]);
            super::return_value(context, u64::from(values.contains(&context.regs[1])));
        }
        StubKind::DictionarySet => {
            let (mut keys, mut values) = dictionary_pairs(mem, context.regs[0]);
            if let Some(index) = keys.iter().position(|key| *key == context.regs[1]) {
                values[index] = context.regs[2];
            } else {
                keys.push(context.regs[1]);
                values.push(context.regs[2]);
            }
            write_dictionary_pairs(mem, context.regs[0], &keys, &values)?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryRemove => {
            let (mut keys, mut values) = dictionary_pairs(mem, context.regs[0]);
            if let Some(index) = keys.iter().position(|key| *key == context.regs[1]) {
                keys.remove(index);
                values.remove(index);
            }
            write_dictionary_pairs(mem, context.regs[0], &keys, &values)?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryRemoveAll => {
            write_dictionary_pairs(mem, context.regs[0], &[], &[])?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryKeysAndValues => {
            let (keys, values) = dictionary_pairs(mem, context.regs[0]);
            for (index, value) in keys.iter().copied().enumerate() {
                if context.regs[1] != 0 {
                    mem.write_u64(context.regs[1] + index as u64 * 8, value)
                        .map_err(str::to_owned)?;
                }
            }
            for (index, value) in values.iter().copied().enumerate() {
                if context.regs[2] != 0 {
                    mem.write_u64(context.regs[2] + index as u64 * 8, value)
                        .map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::StringCreate(utf16) => {
            let value = if symbol == "CFStringCreateMutable" {
                String::new()
            } else if symbol == "CFStringCreate" && utf16 {
                if context.regs[3] == 0x0800_0100 {
                    mem.read_bytes(context.regs[1], context.regs[2].min(1_048_576))
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default()
                } else {
                    string_from_utf16(mem, context.regs[1], context.regs[2])
                }
            } else {
                c_string(mem, context.regs[1])
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default()
            };
            let kind = if symbol == "CFStringCreateMutable" {
                A64_KIND_MUTABLE_STRING
            } else {
                A64_KIND_STRING
            };
            let object = super::objc_string_with_kind(mem, &value, kind)?;
            super::return_value(context, object);
        }
        StubKind::StringCreateCharacters => {
            let value = string_from_utf16(mem, context.regs[1], context.regs[2]);
            let object = super::objc_string_with_kind(mem, &value, A64_KIND_STRING)?;
            super::return_value(context, object);
        }
        StubKind::StringCreateBytes => {
            let value = if context.regs[3] == 0x0800_0100 {
                mem.read_bytes(context.regs[1], context.regs[2].min(1_048_576))
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default()
            } else {
                string_from_utf16(mem, context.regs[1], context.regs[2] / 2)
            };
            let object = super::objc_string_with_kind(mem, &value, A64_KIND_STRING)?;
            super::return_value(context, object);
        }
        StubKind::StringLength => {
            let length = objc_text(mem, context.regs[0])
                .map(|bytes| String::from_utf8_lossy(&bytes).encode_utf16().count() as u64)
                .unwrap_or(0);
            super::return_value(context, length);
        }
        StubKind::StringCharacter => {
            let value = objc_text(mem, context.regs[0])
                .and_then(|bytes| {
                    String::from_utf8_lossy(&bytes)
                        .encode_utf16()
                        .nth(context.regs[1] as usize)
                })
                .unwrap_or(0);
            super::return_value(context, u64::from(value));
        }
        StubKind::StringGetCString => {
            let bytes = objc_text(mem, context.regs[0]).unwrap_or_default();
            let buffer_size = context.regs[2] as usize;
            if context.regs[1] == 0 || buffer_size == 0 {
                super::return_value(context, 0);
            } else {
                let capacity = buffer_size.saturating_sub(1);
                let copied = bytes.len().min(capacity);
                mem.write_bytes(context.regs[1], &bytes[..copied])
                    .map_err(str::to_owned)?;
                mem.write_u8(context.regs[1] + copied as u64, 0)
                    .map_err(str::to_owned)?;
                super::return_value(context, u64::from(copied == bytes.len()));
            }
        }
        StubKind::StringCStringPointer => {
            super::return_value(context, objc_field(mem, context.regs[0], 56));
        }
        StubKind::StringMaximumSize => {
            let length = objc_field(mem, context.regs[0], 56);
            super::return_value(
                context,
                context.regs[0]
                    .saturating_mul(4)
                    .saturating_add(1)
                    .max(length),
            );
        }
        StubKind::StringAppend | StubKind::StringAppendCString => {
            let right = if matches!(kind, StubKind::StringAppendCString) {
                c_string(mem, context.regs[1]).unwrap_or_default()
            } else {
                objc_text(mem, context.regs[1]).unwrap_or_default()
            };
            let mut bytes = objc_text(mem, context.regs[0]).unwrap_or_default();
            bytes.extend(right);
            replace_string(mem, context.regs[0], &bytes)?;
            super::return_value(context, 0);
        }
        StubKind::DataCreate(mutable) => {
            let bytes = if mutable {
                Vec::new()
            } else if context.regs[1] == 0 {
                Vec::new()
            } else {
                mem.read_bytes(context.regs[1], context.regs[2].min(64 * 1024 * 1024))
                    .map_err(str::to_owned)?
            };
            let object = super::objc_object(mem, A64_KIND_DATA)?;
            if !bytes.is_empty() || !mutable {
                replace_data(mem, object, &bytes)?;
            } else {
                set_objc_field(mem, object, 56, 0);
                set_objc_field(mem, object, 64, 0);
            }
            super::return_value(context, object);
        }
        StubKind::DataCreateCopy => {
            let source_pointer = objc_field(mem, context.regs[1], 56);
            let source_length = objc_field(mem, context.regs[1], 64).min(64 * 1024 * 1024);
            let bytes = if source_pointer == 0 || source_length == 0 {
                Vec::new()
            } else {
                mem.read_bytes(source_pointer, source_length)
                    .map_err(str::to_owned)?
            };
            let object = super::objc_object(mem, A64_KIND_DATA)?;
            replace_data(mem, object, &bytes)?;
            super::return_value(context, object);
        }
        StubKind::DataGetBytes => {
            let pointer = objc_field(mem, context.regs[0], 56);
            let length = objc_field(mem, context.regs[0], 64).min(64 * 1024 * 1024) as usize;
            let (location, count) = range_values(mem, context.regs[1], length);
            if context.regs[2] != 0 && count > 0 && pointer != 0 {
                let bytes = mem
                    .read_bytes(pointer + location as u64, count as u64)
                    .map_err(str::to_owned)?;
                mem.write_bytes(context.regs[2], &bytes)
                    .map_err(str::to_owned)?;
            }
            super::return_value(context, 0);
        }
        StubKind::DataBytePointer => {
            super::return_value(context, objc_field(mem, context.regs[0], 56));
        }
        StubKind::DataLength => super::return_value(context, objc_field(mem, context.regs[0], 64)),
        StubKind::DataAppend => {
            let old_pointer = objc_field(mem, context.regs[0], 56);
            let old_length = objc_field(mem, context.regs[0], 64);
            let append_length = context.regs[2].min(64 * 1024 * 1024);
            let mut bytes = if old_pointer == 0 {
                Vec::new()
            } else {
                mem.read_bytes(old_pointer, old_length)
                    .map_err(str::to_owned)?
            };
            if context.regs[1] != 0 && append_length > 0 {
                bytes.extend(
                    mem.read_bytes(context.regs[1], append_length)
                        .map_err(str::to_owned)?,
                );
            }
            replace_data(mem, context.regs[0], &bytes)?;
            super::return_value(context, 0);
        }
        StubKind::NumberCreate => {
            let number_type = context.regs[1];
            let object = if matches!(number_type, 5 | 6 | 12 | 13 | 16) {
                objc_number_float(mem, read_number_as_f64(mem, context.regs[2], number_type))?
            } else {
                objc_number(mem, read_number_as_i64(mem, context.regs[2], number_type))?
            };
            super::return_value(context, object);
        }
        StubKind::NumberValue => {
            if symbol == "CFGetRetainCount" {
                super::return_value(context, 1);
            } else if context.regs[2] != 0 {
                let value = objc_field(mem, context.regs[0], 56);
                let is_float = objc_field(mem, context.regs[0], 64) != 0;
                let number = if is_float {
                    f64::from_bits(value)
                } else {
                    value as i64 as f64
                };
                write_number_value(mem, context.regs[2], context.regs[1], number)?;
                super::return_value(context, 1);
            } else {
                super::return_value(context, 0);
            }
        }
        StubKind::Retain => super::return_value(context, context.regs[0]),
        StubKind::Release => super::return_value(context, 0),
        StubKind::Allocator => {
            let object = objc_object(mem, A64_KIND_GENERIC)?;
            super::return_value(context, object);
        }
        StubKind::Null => {
            let object = objc_object(mem, A64_KIND_GENERIC)?;
            super::return_value(context, object);
        }
        StubKind::GenericPointer => generic_pointer(mem, context)?,
        StubKind::GenericReceiver => super::return_value(context, context.regs[0]),
        StubKind::GenericZero => super::return_value(context, 0),
        StubKind::GxxPersonality => super::return_value(context, 0),
        StubKind::OperatorDeleteNothrow => super::return_value(context, 0),
        StubKind::StdioFilePointer => {
            let file = mem.alloc_zeroed(152).map_err(str::to_owned)?;
            super::return_value(context, file);
        }
        StubKind::ZlibVersion => {
            let version = mem.alloc_zeroed(8).map_err(str::to_owned)?;
            mem.write_bytes(version, b"1.2.11\0")
                .map_err(str::to_owned)?;
            super::return_value(context, version);
        }
        StubKind::Crc32 => {
            let mut crc = context.regs[0] as u32;
            let buffer = context.regs[1];
            let length = context.regs[2];
            if buffer != 0 && length != 0 && mem.allocation_size(buffer).is_some() {
                if let Ok(bytes) = mem.read_bytes(buffer, length.min(64 * 1024 * 1024)) {
                    for byte in bytes {
                        crc = CRC32_TABLE[((crc ^ u32::from(byte)) & 0xff) as usize] ^ (crc >> 8);
                    }
                }
            }
            super::return_value(context, u64::from(crc));
        }
        StubKind::ZlibUncompress => {
            // int uncompress(Bytef *dest, uLongf *destLen, const Bytef *src, uLong srcLen)
            let dest = context.regs[0];
            let dest_len_pointer = context.regs[1];
            let source = context.regs[2];
            let source_length = context.regs[3];
            if dest == 0 || dest_len_pointer == 0 || source == 0 {
                super::return_value(context, 0xfffffffau64); // Z_DATA_ERROR
                return Ok(true);
            }
            let capacity = mem
                .read_u64(dest_len_pointer)
                .unwrap_or(0)
                .min(64 * 1024 * 1024);
            let compressed = mem
                .read_bytes(source, source_length.min(64 * 1024 * 1024))
                .map_err(str::to_owned)?;
            let mut decompressor = flate2::Decompress::new(true);
            let mut output = vec![0u8; capacity as usize];
            match decompressor.decompress(&compressed, &mut output, flate2::FlushDecompress::Finish)
            {
                Ok(_) => {
                    let produced = decompressor.total_out() as usize;
                    mem.write_bytes(dest, &output[..produced])
                        .map_err(str::to_owned)?;
                    mem.write_u64(dest_len_pointer, produced as u64)
                        .map_err(str::to_owned)?;
                    super::return_value(context, 0); // Z_OK
                }
                Err(_) => super::return_value(context, 0xfffffffdu64), // Z_BUF_ERROR
            }
        }
        StubKind::ZlibInit | StubKind::GzClose | StubKind::SemTimedwait | StubKind::SetIOPolicy => {
            super::return_value(context, 0);
        }
        StubKind::GzOpen => {
            // gzopen cannot reach guest files without the FS translation
            // layer; a handle that reads as EOF beats crashing the caller.
            let handle = mem.alloc_zeroed(32).map_err(str::to_owned)?;
            super::return_value(context, handle);
        }
        StubKind::GzRead => super::return_value(context, 0),
        StubKind::Dup2 => super::return_value(context, context.regs[1]),
        StubKind::Execl | StubKind::Socketpair => {
            super::return_value(context, (-1i64) as u64);
        }
        StubKind::TcGetSetAttr => {
            let buffer = context.regs[1];
            if buffer != 0 && mem.allocation_size(buffer).is_some() {
                let _ = mem.write_bytes(buffer, &vec![0u8; 64]);
            }
            super::return_value(context, 0);
        }
        StubKind::MethodGetNumberOfArguments => {
            let method = context.regs[0];
            // `Method` points at a struct whose first field is the `types`
            // encoding string.
            let types_pointer = objc_field(mem, method, 0);
            let encoding = c_string(mem, types_pointer).unwrap_or_default();
            super::return_value(context, u64::from(objc_encoding_argument_count(&encoding)));
        }
        StubKind::ObjectSetClass => {
            let object = context.regs[0];
            let new_class = context.regs[1];
            let previous = objc_field(mem, object, 0);
            if object != 0 && mem.allocation_size(object).is_some() {
                set_objc_field(mem, object, 0, new_class);
            }
            super::return_value(context, previous);
        }
        StubKind::Basename => {
            let path = c_string(mem, context.regs[0]).unwrap_or_default();
            let name = path
                .rsplit(|&byte| byte == b'/')
                .next()
                .unwrap_or(&path)
                .to_vec();
            let copy = mem
                .alloc_zeroed(name.len() as u64 + 1)
                .map_err(str::to_owned)?;
            mem.write_bytes(copy, &name).map_err(str::to_owned)?;
            mem.write_u8(copy + name.len() as u64, 0)
                .map_err(str::to_owned)?;
            super::return_value(context, copy);
        }
        StubKind::AUGraphGetNodeCount => {
            let out = context.regs[1];
            if out != 0 && mem.allocation_size(out).is_some() {
                mem.write_u32(out, 0).map_err(str::to_owned)?;
            }
            super::return_value(context, 0);
        }
        StubKind::AudioQueueDeviceGetCurrentTime => {
            let out = context.regs[1];
            if out != 0 && mem.allocation_size(out).is_some() {
                let sample = GUEST_SAMPLE_CLOCK.fetch_add(1024, Ordering::Relaxed);
                // AudioTimeStamp: mSampleTime f64, mHostTime u64,
                // mRateScalar f64, mWordClockTime u64, mFlags u32.
                mem.write_u64(out, (sample as f64).to_bits())
                    .map_err(str::to_owned)?;
                mem.write_u64(out + 8, 0).map_err(str::to_owned)?;
                mem.write_u64(out + 16, 1.0f64.to_bits())
                    .map_err(str::to_owned)?;
                mem.write_u64(out + 24, 0).map_err(str::to_owned)?;
                mem.write_u32(out + 32, 1).map_err(str::to_owned)?; // kAudioTimeStampSampleTimeValid
            }
            super::return_value(context, 0);
        }
        StubKind::AudioQueueOfflineRender => {
            // (AudioQueueRef, const AudioTimeStamp*, AudioBufferList*,
            //  UInt32 inNumberFrames). Silence the destination buffers.
            let buffer_list = context.regs[2];
            if buffer_list != 0 && mem.allocation_size(buffer_list).is_some() {
                let number_buffers = mem.read_u32(buffer_list).unwrap_or(0).min(8);
                for index in 0..number_buffers {
                    let buffer = buffer_list + 8 + u64::from(index) * 16;
                    let byte_size = mem.read_u32(buffer + 4).unwrap_or(0);
                    let data = mem.read_u64(buffer + 8).unwrap_or(0);
                    if data != 0 && byte_size != 0 && mem.allocation_size(data).is_some() {
                        let _ = mem.write_bytes(data, &vec![0u8; byte_size as usize]);
                    }
                }
            }
            super::return_value(context, 0);
        }
        StubKind::CMTimeAdd | StubKind::CMTimeSubtract => {
            let subtract = matches!(kind, StubKind::CMTimeSubtract);
            let a = read_cm_time(mem, context.regs[0]);
            let b = read_cm_time(mem, context.regs[1]);
            let timescale = lcm_timescale(a.timescale, b.timescale);
            let scaled_a = if a.timescale == 0 {
                0i64
            } else {
                a.value * (timescale / i64::from(a.timescale))
            };
            let scaled_b = if b.timescale == 0 {
                0i64
            } else {
                b.value * (timescale / i64::from(b.timescale))
            };
            let value = if subtract {
                scaled_a - scaled_b
            } else {
                scaled_a + scaled_b
            };
            write_cm_time(
                context,
                mem,
                value,
                timescale as i32,
                0x1,
                a.epoch.max(b.epoch),
            )?;
        }
        StubKind::CMTimeCompare => {
            let a = read_cm_time(mem, context.regs[0]);
            let b = read_cm_time(mem, context.regs[1]);
            let seconds_a = cm_time_seconds(&a);
            let seconds_b = cm_time_seconds(&b);
            let result: i64 = if seconds_a < seconds_b {
                -1
            } else if seconds_a > seconds_b {
                1
            } else {
                0
            };
            super::return_value(context, result as u64);
        }
        StubKind::CMTimeConvertScale => {
            let time = read_cm_time(mem, context.regs[0]);
            let new_timescale = context.regs[1] as i32;
            let seconds = cm_time_seconds(&time);
            let value = if new_timescale == 0 {
                0
            } else {
                (seconds * new_timescale as f64).round() as i64
            };
            write_cm_time(context, mem, value, new_timescale, 0x1, time.epoch)?;
        }
        StubKind::CMTimeRangeMake => {
            let start = read_cm_time(mem, context.regs[0]);
            let duration = read_cm_time(mem, context.regs[1]);
            let out = context.regs[8];
            if out != 0 && mem.allocation_size(out).is_some() {
                write_cm_time_at(
                    mem,
                    out,
                    start.value,
                    start.timescale,
                    start.flags,
                    start.epoch,
                )?;
                write_cm_time_at(
                    mem,
                    out + 24,
                    duration.value,
                    duration.timescale,
                    duration.flags,
                    duration.epoch,
                )?;
            }
            super::return_value(context, out);
        }
        StubKind::CMTimeRangeContainsTime => {
            let range_start = read_cm_time(mem, context.regs[0]);
            let range_duration = read_cm_time(mem, context.regs[0] + 24);
            let time = read_cm_time(mem, context.regs[1]);
            let start = cm_time_seconds(&range_start);
            let end = start + cm_time_seconds(&range_duration);
            let point = cm_time_seconds(&time);
            super::return_value(context, u64::from(point >= start && point < end));
        }
        StubKind::CMTimeCopyDescription => {
            let time = read_cm_time(mem, context.regs[1]);
            let text = format!(
                "CMTime {{{}/{}s + {}}}",
                time.value, time.timescale, time.epoch
            );
            let string = super::objc_object(mem, A64_KIND_STRING)?;
            replace_string(mem, string, text.as_bytes())?;
            super::return_value(context, string);
        }
        StubKind::CMSampleBufferCreate => {
            // (allocator, blockBuffer, dataReady, callback, refcon,
            //  formatDesc, numSamples, numTimingEntries, timingArray,
            //  sizeArray, **out). Store PTS + flags so the Get* accessors
            //  return plausible data.
            let block_buffer = context.regs[1];
            let data_ready = context.regs[2];
            let format_description = context.regs[5];
            let timing_array = context.regs[8];
            let sample_buffer = mem.alloc_zeroed(96).map_err(str::to_owned)?;
            // Slots: [0..24]=PTS CMTime, [24]=dataReady, [32]=formatDesc,
            // [40]=blockBuffer, [48]=dataReadyFlag.
            let timing = read_cm_time(mem, timing_array);
            write_cm_time_at(
                mem,
                sample_buffer,
                timing.value,
                timing.timescale,
                timing.flags,
                timing.epoch,
            )?;
            mem.write_u8(sample_buffer + 24, data_ready as u8)
                .map_err(str::to_owned)?;
            mem.write_u64(sample_buffer + 32, format_description)
                .map_err(str::to_owned)?;
            mem.write_u64(sample_buffer + 40, block_buffer)
                .map_err(str::to_owned)?;
            mem.write_u8(sample_buffer + 48, data_ready as u8)
                .map_err(str::to_owned)?;
            if let Some(out) = stack_arg(context, 2) {
                if out != 0 && mem.allocation_size(out).is_some() {
                    mem.write_u64(out, sample_buffer).map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0); // OSStatus noErr
        }
        StubKind::CMSampleBufferGetPresentationTimeStamp => {
            let sample_buffer = context.regs[0];
            let timing = read_cm_time(mem, sample_buffer);
            let out = context.regs[8];
            if out != 0 && mem.allocation_size(out).is_some() {
                write_cm_time_at(
                    mem,
                    out,
                    timing.value,
                    timing.timescale,
                    timing.flags,
                    timing.epoch,
                )?;
            }
            super::return_value(context, out);
        }
        StubKind::CMSampleBufferGetSampleTimingInfo => {
            // (CMSampleBufferRef, CMItemCount index, CMSampleTimingInfo *out)
            let sample_buffer = context.regs[0];
            let out = context.regs[2];
            if out != 0 && mem.allocation_size(out).is_some() {
                if let Ok(bytes) = mem.read_bytes(sample_buffer, 24) {
                    mem.write_bytes(out, &bytes).map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::CMSampleBufferGetFormatDescription => {
            let sample_buffer = context.regs[0];
            let description = objc_field(mem, sample_buffer, 32);
            super::return_value(context, description);
        }
        StubKind::CMSampleBufferCreateCopyWithNewTiming => {
            // (allocator, originalBuffer, numTimingEntries, timingArray,
            //  sizeArray, **out)
            let original = context.regs[1];
            let copy = mem.alloc_zeroed(96).map_err(str::to_owned)?;
            if original != 0 && mem.allocation_size(original).is_some() {
                if let Ok(bytes) = mem.read_bytes(original, 96) {
                    mem.write_bytes(copy, &bytes).map_err(str::to_owned)?;
                }
            }
            if let Some(out) = stack_arg(context, 3) {
                if out != 0 && mem.allocation_size(out).is_some() {
                    mem.write_u64(out, copy).map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::CMSampleBufferSetDataBufferFromAudioBufferList
        | StubKind::CMSampleBufferSetDataReady => super::return_value(context, 0),
        StubKind::CMAudioFormatDescriptionCreate => {
            // (allocator, asbd*, layoutSize, layout*, magicCookieSize,
            //  magicCookie, extensions, out**). Store the ASBD at [0..40]
            //  and the channel layout at [64..128].
            let asbd_pointer = context.regs[1];
            let layout_size = context.regs[2];
            let description = mem.alloc_zeroed(128).map_err(str::to_owned)?;
            if asbd_pointer != 0 && mem.allocation_size(asbd_pointer).is_some() {
                if let Ok(bytes) = mem.read_bytes(asbd_pointer, 40) {
                    mem.write_bytes(description, &bytes)
                        .map_err(str::to_owned)?;
                }
            }
            if layout_size != 0 && context.regs[3] != 0 {
                let layout_size = layout_size.min(4096);
                let layout = mem.alloc_zeroed(layout_size).map_err(str::to_owned)?;
                if let Ok(bytes) = mem.read_bytes(context.regs[3], layout_size) {
                    let _ = mem.write_bytes(layout, &bytes);
                }
                mem.write_u32(description + 64, layout_size as u32)
                    .map_err(str::to_owned)?;
                mem.write_u64(description + 72, layout)
                    .map_err(str::to_owned)?;
            }
            if let Some(out) = stack_arg(context, 1) {
                if out != 0 && mem.allocation_size(out).is_some() {
                    mem.write_u64(out, description).map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::CMAudioFormatDescriptionGetStreamBasicDescription => {
            super::return_value(context, context.regs[0]);
        }
        StubKind::CMAudioFormatDescriptionGetChannelLayout => {
            let description = context.regs[0];
            let layout = objc_field(mem, description, 72);
            super::return_value(context, layout);
        }
        StubKind::CVMetalTextureGetTexture => super::return_value(context, 0),
        StubKind::UnwindResume => {
            // Landing here means guest C++ exceptions unwound through a host
            // frame. Returning zero gives the app a chance to continue; a
            // hard abort here would take down the whole process.
            super::return_value(context, 0);
        }
        StubKind::CMSampleBufferGetDataIsReady => {
            let ready = mem
                .read_u32(context.regs[0].saturating_add(24))
                .unwrap_or(0);
            super::return_value(context, u64::from(ready != 0));
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corefoundation_collection_stubs_preserve_guest_values() {
        let mut memory = Mem64::new();
        let first = objc_object(&mut memory, A64_KIND_GENERIC).unwrap();
        let second = objc_object(&mut memory, A64_KIND_GENERIC).unwrap();
        let values = memory.alloc_zeroed(16).unwrap();
        memory.write_u64(values, first).unwrap();
        memory.write_u64(values + 8, second).unwrap();
        let mut context = touchHLE_DynarmicA64Context::default();
        context.regs[1] = values;
        context.regs[2] = 2;
        dispatch(&mut memory, &mut context, "CFArrayCreate").unwrap();
        let array = context.regs[0];
        context.regs[0] = array;
        dispatch(&mut memory, &mut context, "CFArrayGetCount").unwrap();
        assert_eq!(context.regs[0], 2);
        context.regs[0] = array;
        context.regs[1] = 1;
        dispatch(&mut memory, &mut context, "CFArrayGetValueAtIndex").unwrap();
        assert_eq!(context.regs[0], second);
    }

    #[test]
    fn corefoundation_string_and_data_stubs_preserve_guest_values() {
        let mut memory = Mem64::new();
        let text = memory.alloc_zeroed(6).unwrap();
        memory.write_bytes(text, b"hello").unwrap();
        let mut context = touchHLE_DynarmicA64Context::default();
        context.regs[1] = text;
        dispatch(&mut memory, &mut context, "CFStringCreateWithCString").unwrap();
        let string = context.regs[0];
        assert_eq!(objc_text(&memory, string).as_deref(), Some(&b"hello"[..]));
        context.regs[0] = string;
        dispatch(&mut memory, &mut context, "CFStringGetLength").unwrap();
        assert_eq!(context.regs[0], 5);

        let bytes = memory.alloc_zeroed(3).unwrap();
        memory.write_bytes(bytes, &[1, 2, 3]).unwrap();
        context.regs[1] = bytes;
        context.regs[2] = 3;
        dispatch(&mut memory, &mut context, "CFDataCreate").unwrap();
        let data = context.regs[0];
        context.regs[0] = data;
        dispatch(&mut memory, &mut context, "CFDataGetLength").unwrap();
        assert_eq!(context.regs[0], 3);
    }
}
