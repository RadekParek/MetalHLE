/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIResponder`.

use crate::objc::{id, msg, nil, objc_classes, ClassExports};

#[derive(Default)]
pub struct State {
    pub first_responder: id,
}

// Cocos2d's iOS EAGLView is itself a UIKeyInput responder. Its guest methods
// forward host keyboard text to Cocos2d's IME dispatcher.
pub fn is_text_input_responder(env: &mut crate::Environment, responder: id) -> bool {
    if responder == nil {
        return false;
    }
    let insert_text = env
        .objc
        .register_host_selector("insertText:".to_string(), &mut env.mem);
    let delete_backward = env
        .objc
        .register_host_selector("deleteBackward".to_string(), &mut env.mem);
    msg![env; responder respondsToSelector:insert_text]
        && msg![env; responder respondsToSelector:delete_backward]
}

pub fn handle_text_input_event(
    env: &mut crate::Environment,
    responder: id,
    event: crate::window::TextInputEvent,
) {
    match event {
        crate::window::TextInputEvent::Text(text) => {
            let text = crate::frameworks::foundation::ns_string::from_rust_string(env, text);
            let _: () = msg![env; responder insertText:text];
            crate::objc::release(env, text);
        }
        crate::window::TextInputEvent::Backspace => {
            let _: () = msg![env; responder deleteBackward];
        }
        crate::window::TextInputEvent::Return => {
            let newline =
                crate::frameworks::foundation::ns_string::from_rust_string(env, "\n".to_string());
            let _: () = msg![env; responder insertText:newline];
            crate::objc::release(env, newline);
        }
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIResponder: NSObject

// TODO: real responder implementation etc

// The default implementation of these methods forward the message
// up the responder chain

- (())touchesBegan:(id)touches // NSSet* of UITouch*
         withEvent:(id)event { // UIEvent*
    log_dbg!(
        "[{:?} touchesBegan:{:?} withEvent:{:?}] (probably unhandled)",
        this,
        touches,
        event,
    );
    let next_responder: id = msg![env; this nextResponder];
    if next_responder != nil {
        () = msg![env; next_responder touchesBegan:touches withEvent:event];
    }
}

- (())touchesMoved:(id)touches // NSSet* of UITouch*
         withEvent:(id)event { // UIEvent*
    log_dbg!(
        "[{:?} touchesMoved:{:?} withEvent:{:?}] (probably unhandled)",
        this,
        touches,
        event,
    );
    let next_responder: id = msg![env; this nextResponder];
    if next_responder != nil {
        () = msg![env; next_responder touchesMoved:touches withEvent:event];
    }
}

- (())touchesEnded:(id)touches // NSSet* of UITouch*
         withEvent:(id)event { // UIEvent*
    log_dbg!(
        "[{:?} touchesEnded:{:?} withEvent:{:?}] (probably unhandled)",
        this,
        touches,
        event,
    );
    let next_responder: id = msg![env; this nextResponder];
    if next_responder != nil {
        () = msg![env; next_responder touchesEnded:touches withEvent:event];
    }
}

- (id)nextResponder {
    nil
}

- (bool)isFirstResponder {
    env.framework_state.uikit.ui_responder.first_responder == this
}

- (bool)canBecomeFirstResponder {
    false
}

- (bool)becomeFirstResponder {
    if !msg![env; this canBecomeFirstResponder] {
        return false;
    }

    let accepts_text = is_text_input_responder(env, this);
    let already_first_responder = env.framework_state.uikit.ui_responder.first_responder == this;
    let previous_responder = env.framework_state.uikit.ui_responder.first_responder;
    if !already_first_responder && previous_responder != nil {
        if !msg![env; previous_responder resignFirstResponder] {
            return false;
        }
    }

    env.framework_state.uikit.ui_responder.first_responder = this;
    if accepts_text {
        if !already_first_responder {
            crate::frameworks::uikit::ui_keyboard::post_keyboard_notifications(env, true);
        }
        crate::frameworks::uikit::ui_keyboard::start_text_input(env);
    }
    true
}

- (bool)canResignFirstResponder {
    true
}

- (bool)resignFirstResponder {
    if env.framework_state.uikit.ui_responder.first_responder == this {
        if !msg![env; this canResignFirstResponder] {
            return false;
        }
        let accepts_text = is_text_input_responder(env, this);
        env.framework_state.uikit.ui_responder.first_responder = nil;
        if accepts_text {
            crate::frameworks::uikit::ui_keyboard::post_keyboard_notifications(env, false);
            crate::frameworks::uikit::ui_keyboard::stop_text_input(env);
        }
    }
    true
}

@end

};
