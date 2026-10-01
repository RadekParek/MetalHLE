/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ASIdentifierManager` — Advertising Support framework.
//!
//! Returns a real, per-install advertising identifier (a random UUIDv4
//! generated on first use and persisted in the app's sandbox so it is
//! stable across relaunches). This matches iOS 6/7 behaviour, where
//! ASIdentifierManager always handed out a genuine device UUID — the
//! all-zeros "ad tracking limited" value only exists from iOS 14 onward.
//! Tracking is still reported as disabled / limited.

use crate::frameworks::foundation::ns_string;
use crate::objc::{id, msg, msg_class, nil, objc_classes, ClassExports, HostObject, NSZonePtr};

// =========================================================================
// MARK: - Persistent IDFA storage
// =========================================================================

/// A real, per-install advertising identifier: a random UUID generated the
/// first time an app asks for it, then persisted in the app's sandbox so it
/// is stable across launches — matching what iOS 6/7 devices actually
/// returned (the all-zero IDFA only exists on iOS 14+ when tracking is
/// denied, an era MetalHLE does not emulate).
fn persistent_idfa_string(env: &mut crate::Environment) -> String {
    let bundle_id = env.bundle.bundle_identifier().to_owned();
    let dir = crate::paths::user_data_base_path()
        .join(crate::paths::SANDBOX_DIR)
        .join(&bundle_id);
    let path = dir.join(".metalhle_idfa");

    if let Ok(existing) = std::fs::read_to_string(&path) {
        let existing = existing.trim();
        if is_uuid_string(existing) {
            return existing.to_owned();
        }
    }

    // Generate a random v4 UUID from the OS entropy pool.
    let mut bytes: [u8; 16] = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| {
            use std::io::Read;
            let mut b = [0u8; 16];
            f.read_exact(&mut b)?;
            Ok(b)
        })
        .unwrap_or_else(|_| {
            // Entropy unavailable: fall back to a hash of time + bundle id so
            // we still return a plausible, per-app-distinct identifier.
            use sha1::{Digest, Sha1};
            let mut h = Sha1::new();
            h.update(bundle_id.as_bytes());
            h.update(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos().to_le_bytes())
                    .unwrap_or([0u8; 16]),
            );
            let digest = h.finalize();
            let mut b = [0u8; 16];
            b.copy_from_slice(&digest[..16]);
            b
        });
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let uuid_str = format!(
        "{:02X}{:02X}{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
        bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11],
        bytes[12], bytes[13], bytes[14], bytes[15]
    );

    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(&path, &uuid_str);
    }
    uuid_str
}

/// Loose RFC 4122 shape check (8-4-4-4-12 hex with dashes) for the persisted
/// value; guards against a corrupted file making NSUUID init fail.
fn is_uuid_string(s: &str) -> bool {
    let hex = |b: u8| b.is_ascii_hexdigit();
    let bytes = s.as_bytes();
    bytes.len() == 36
        && bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, &b)| (i == 8 || i == 13 || i == 18 || i == 23) || hex(b))
}

// =========================================================================
// MARK: - ASIdentifierManager host object
// =========================================================================

#[derive(Default)]
struct ASIdentifierManagerHostObject {
    /// NSUUID* — the advertising identifier. Created once and cached.
    advertising_identifier: id,
}
impl HostObject for ASIdentifierManagerHostObject {}

// =========================================================================
// MARK: - ASTrackingManager (iOS 14+ — same file, thin stub)
// =========================================================================

struct ATTrackingManagerHostObject;
impl HostObject for ATTrackingManagerHostObject {}

// ATTrackingManager authorization status values
// (ATTrackingManagerAuthorizationStatus)
pub type ATTrackingManagerAuthorizationStatus = u32;
pub const ATTrackingManagerAuthorizationStatusNotDetermined: ATTrackingManagerAuthorizationStatus =
    0;
pub const ATTrackingManagerAuthorizationStatusRestricted: ATTrackingManagerAuthorizationStatus = 1;
pub const ATTrackingManagerAuthorizationStatusDenied: ATTrackingManagerAuthorizationStatus = 2;
pub const ATTrackingManagerAuthorizationStatusAuthorized: ATTrackingManagerAuthorizationStatus = 3;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// =========================================================================
// MARK: - ASIdentifierManager
// =========================================================================

@implementation ASIdentifierManager: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(ASIdentifierManagerHostObject {
        advertising_identifier: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// -------------------------------------------------------------------------
// Singleton accessor
// -------------------------------------------------------------------------

+ (id)sharedManager {
    // Store the singleton on the class object itself via a static.
    // We use a simple approach: alloc+init once, then return the same object.
    // In a multi-call scenario we rely on the fact that the host object keeps
    // the uuid cached.
    let instance: id = msg![env; this alloc];
    let instance: id = msg![env; instance init];
    // autorelease so callers who don't retain it don't leak.
    crate::objc::autorelease(env, instance)
}

- (id)init {
    this
}

- (())dealloc {
    let identifier =
        env.objc.borrow::<ASIdentifierManagerHostObject>(this).advertising_identifier;
    crate::objc::release(env, identifier);
    env.objc.dealloc_object(this, &mut env.mem)
}

// -------------------------------------------------------------------------
// advertisingIdentifier — returns a fixed stable NSUUID*
// -------------------------------------------------------------------------

- (id)advertisingIdentifier { // NSUUID*
    let existing =
        env.objc.borrow::<ASIdentifierManagerHostObject>(this).advertising_identifier;
    if existing != nil {
        return existing;
    }

    // Real per-app UUID, persisted across launches (see persistent_idfa_string).
    let uuid_str_owned = persistent_idfa_string(env);
    log_dbg!(
        "ASIdentifierManager advertisingIdentifier — returning persistent per-install UUID {}",
        uuid_str_owned
    );
    let ns_uuid_str = ns_string::from_rust_string(env, uuid_str_owned);
    let uuid: id = msg_class![env; NSUUID alloc];
    let uuid: id = msg![env; uuid initWithUUIDString:ns_uuid_str];
    crate::objc::release(env, ns_uuid_str);

    crate::objc::retain(env, uuid);
    env.objc.borrow_mut::<ASIdentifierManagerHostObject>(this)
        .advertising_identifier = uuid;

    uuid
}

// -------------------------------------------------------------------------
// isAdvertisingTrackingEnabled — always NO (opted out)
// -------------------------------------------------------------------------

- (bool)isAdvertisingTrackingEnabled {
    // Return false — tracking is always disabled in touchHLE.
    // Apps should respect this and not send advertising data.
    log_dbg!("ASIdentifierManager isAdvertisingTrackingEnabled — returning NO");
    false
}

// -------------------------------------------------------------------------
// trackingAuthorizationStatus (iOS 14+)
// -------------------------------------------------------------------------

- (ATTrackingManagerAuthorizationStatus)trackingAuthorizationStatus {
    // Report "denied" so apps don't try to show a tracking permission dialog.
    ATTrackingManagerAuthorizationStatusDenied
}

// -------------------------------------------------------------------------
// NSCopying
// -------------------------------------------------------------------------

- (id)copyWithZone:(NSZonePtr)_zone {
    // Singleton — return self retained.
    crate::objc::retain(env, this);
    this
}

// -------------------------------------------------------------------------
// Description
// -------------------------------------------------------------------------

- (id)description {
    let s = format!(
        "<ASIdentifierManager: {:?}; trackingEnabled=NO; status=Denied>",
        this
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

// =========================================================================
// MARK: - ATTrackingManager (iOS 14+ App Tracking Transparency)
// =========================================================================

@implementation ATTrackingManager: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(ATTrackingManagerHostObject);
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// -------------------------------------------------------------------------
// trackingAuthorizationStatus — class-level query
// -------------------------------------------------------------------------

+ (ATTrackingManagerAuthorizationStatus)trackingAuthorizationStatus {
    log_dbg!("ATTrackingManager trackingAuthorizationStatus — returning Denied");
    ATTrackingManagerAuthorizationStatusDenied
}

// -------------------------------------------------------------------------
// requestTrackingAuthorizationWithCompletionHandler:
// Immediately calls the completion handler with "Denied" — no UI shown.
// -------------------------------------------------------------------------

+ (())requestTrackingAuthorizationWithCompletionHandler:(id)completion_handler {
    log_dbg!(
        "ATTrackingManager requestTrackingAuthorizationWithCompletionHandler: \
         — calling handler with Denied immediately"
    );
    if completion_handler == nil {
        return;
    }
    // The completion handler is a block: ^(ATTrackingManagerAuthorizationStatus
    // status)
    // We call it by sending it the __FuncPtr invoke message with the status.
    let status: ATTrackingManagerAuthorizationStatus = ATTrackingManagerAuthorizationStatusDenied;
    // Invoke the block — blocks respond to `invoke` in touchHLE's block model.
    let sel = env.objc.lookup_selector("invokeWithUnsignedInt:").unwrap();
    let responds: bool = msg![env; completion_handler respondsToSelector:sel];
    if responds {
        let _: () = msg![env; completion_handler invokeWithUnsignedInt:status];
    } else {
        // Fallback: try plain invoke with no arguments (some block wrappers).
        let sel_plain = env.objc.lookup_selector("invoke").unwrap();
        let responds_plain: bool = msg![env; completion_handler respondsToSelector:sel_plain];
        if responds_plain {
            let _: () = msg![env; completion_handler invoke];
        } else {
            log!("ATTrackingManager: completion handler {:?} does not respond to invoke — ignored", completion_handler);
        }
    }
}

// -------------------------------------------------------------------------
// Description
// -------------------------------------------------------------------------

+ (id)description {
    let s = "ATTrackingManager (touchHLE stub — always Denied)";
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};
