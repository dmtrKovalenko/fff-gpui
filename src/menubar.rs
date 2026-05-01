#[cfg(target_os = "macos")]
mod mac {
    #![allow(unexpected_cfgs, unsafe_op_in_unsafe_fn)]

    use std::cell::RefCell;
    use std::rc::{Rc, Weak};

    use async_channel::Sender;
    use cocoa::{
        appkit::{
            NSApp, NSButton, NSEventMask, NSMenu, NSMenuItem, NSSquareStatusItemLength,
            NSStatusBar, NSStatusItem, NSView,
        },
        base::{NO, id, nil},
        foundation::{NSPoint, NSRect, NSString},
    };
    use objc::{
        class,
        declare::ClassDecl,
        msg_send,
        rc::StrongPtr,
        runtime::{Class, Object, Sel},
        sel, sel_impl,
    };

    use crate::service::ServiceCommand;

    thread_local! {
        static STATUS_ITEM: RefCell<Option<StatusItemHandle>> = const { RefCell::new(None) };
    }

    static mut VIEW_CLASS: *const Class = core::ptr::null();
    const STATE_IVAR: &str = "state";

    pub fn install(commands: Sender<ServiceCommand>) {
        STATUS_ITEM.with(|slot| {
            if slot.borrow().is_none() {
                slot.borrow_mut()
                    .replace(unsafe { StatusItemHandle::new(commands) });
            }
        });
    }

    struct StatusItemHandle {
        _native_item: StrongPtr,
        _target: StrongPtr,
        _state: Rc<RefCell<StatusItemState>>,
    }

    struct StatusItemState {
        commands: Sender<ServiceCommand>,
        status_item: id,
    }

    impl StatusItemHandle {
        unsafe fn new(commands: Sender<ServiceCommand>) -> Self {
            ensure_view_class();

            let status_bar = NSStatusBar::systemStatusBar(nil);
            let native_item =
                StrongPtr::retain(status_bar.statusItemWithLength_(NSSquareStatusItemLength));
            let button = native_item.button();
            button.setTitle_(NSString::alloc(nil).init_str("🪿"));
            let _: () = msg_send![button, setToolTip: NSString::alloc(nil).init_str("fff-gpui")];

            let state = Rc::new(RefCell::new(StatusItemState {
                commands,
                status_item: *native_item,
            }));
            let target: id = msg_send![VIEW_CLASS, alloc];
            NSView::initWithFrame_(
                target,
                NSRect::new(NSPoint::new(0., 0.), button.frame().size),
            );
            (*target).set_ivar(
                STATE_IVAR,
                Weak::into_raw(Rc::downgrade(&state)) as *const core::ffi::c_void,
            );
            NSButton::setTarget_(button, target);
            button.setAction_(sel!(statusItemClicked:));
            let _: () = msg_send![
                button,
                sendActionOn: NSEventMask::NSLeftMouseUpMask | NSEventMask::NSRightMouseUpMask
            ];

            Self {
                _native_item: native_item,
                _target: StrongPtr::new(target),
                _state: state,
            }
        }
    }

    unsafe fn ensure_view_class() {
        if VIEW_CLASS.is_null() {
            let mut decl = ClassDecl::new("FFFStatusItemView", class!(NSView)).unwrap();
            decl.add_ivar::<*mut core::ffi::c_void>(STATE_IVAR);
            decl.add_method(sel!(dealloc), dealloc_view as extern "C" fn(&Object, Sel));
            decl.add_method(
                sel!(statusItemClicked:),
                status_item_clicked as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(openMenuItemClicked:),
                open_menu_item_clicked as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(
                sel!(quitMenuItemClicked:),
                quit_menu_item_clicked as extern "C" fn(&Object, Sel, id),
            );
            VIEW_CLASS = decl.register();
        }
    }

    extern "C" fn status_item_clicked(this: &Object, _: Sel, _: id) {
        unsafe {
            if let Some(state) = get_state(this).upgrade() {
                let state_ref = state.borrow();
                let commands = state_ref.commands.clone();
                let event: id = msg_send![NSApp(), currentEvent];
                let button_number: isize = if event == nil {
                    0
                } else {
                    msg_send![event, buttonNumber]
                };

                if button_number == 1 {
                    let menu = build_menu(this);
                    let _: () = msg_send![state_ref.status_item, popUpStatusItemMenu: menu];
                } else {
                    let _ = commands.send_blocking(ServiceCommand::ToggleWindow);
                }
            }
        }
    }

    extern "C" fn open_menu_item_clicked(this: &Object, _: Sel, _: id) {
        unsafe {
            if let Some(state) = get_state(this).upgrade() {
                let commands = state.borrow().commands.clone();
                let _ = commands.send_blocking(ServiceCommand::ShowPicker);
            }
        }
    }

    #[allow(dead_code)]
    extern "C" fn config_menu_item_clicked(this: &Object, _: Sel, _: id) {
        unsafe {
            if let Some(state) = get_state(this).upgrade() {
                let commands = state.borrow().commands.clone();
                let _ = commands.send_blocking(ServiceCommand::OpenConfig);
            }
        }
    }

    extern "C" fn quit_menu_item_clicked(this: &Object, _: Sel, _: id) {
        unsafe {
            if let Some(state) = get_state(this).upgrade() {
                let commands = state.borrow().commands.clone();
                let _ = commands.send_blocking(ServiceCommand::Quit);
            }
        }
    }

    unsafe fn build_menu(this: &Object) -> id {
        let menu = NSMenu::new(nil);
        menu.setAutoenablesItems(NO);
        let open_title = NSString::alloc(nil).init_str("Open");
        let config_title = NSString::alloc(nil).init_str("Open Config");
        let quit_title = NSString::alloc(nil).init_str("Quit");
        let empty = NSString::alloc(nil).init_str("");
        let open_item = NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(
            open_title,
            sel!(openMenuItemClicked:),
            empty,
        );
        NSMenuItem::setTarget_(open_item, this as *const _ as id);
        let config_item = NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(
            config_title,
            sel!(configMenuItemClicked:),
            empty,
        );
        NSMenuItem::setTarget_(config_item, this as *const _ as id);
        let quit_item = NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(
            quit_title,
            sel!(quitMenuItemClicked:),
            empty,
        );
        NSMenuItem::setTarget_(quit_item, this as *const _ as id);

        menu.addItem_(open_item);
        menu.addItem_(config_item);
        menu.addItem_(quit_item);
        menu
    }

    extern "C" fn dealloc_view(this: &Object, _: Sel) {
        unsafe {
            drop_state(this);
            let _: () = msg_send![super(this, class!(NSView)), dealloc];
        }
    }

    unsafe fn get_state(object: &Object) -> Weak<RefCell<StatusItemState>> {
        let raw: *mut core::ffi::c_void = *object.get_ivar(STATE_IVAR);
        let weak1 = Weak::from_raw(raw as *mut RefCell<StatusItemState>);
        let weak2 = weak1.clone();
        let _ = Weak::into_raw(weak1);
        weak2
    }

    unsafe fn drop_state(object: &Object) {
        let raw: *const core::ffi::c_void = *object.get_ivar(STATE_IVAR);
        Weak::from_raw(raw as *const RefCell<StatusItemState>);
    }
}

#[cfg(target_os = "macos")]
pub use mac::install;

#[cfg(not(target_os = "macos"))]
pub fn install(_: async_channel::Sender<crate::service::ServiceCommand>) {}
