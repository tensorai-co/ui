//! Raw C-ABI FFI bindings, API hash verification, and runtime asset lookup for Spotify CEF.

#![allow(
    non_camel_case_types,
    non_snake_case,
    dead_code,
    clippy::type_complexity
)]

use std::ffi::{CStr, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/cef_generated.rs"));

/// `char16_t` on LP64 / LLP64 targets.
pub type char16_t = u16;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct cef_string_utf16_t {
    pub str: *mut char16_t,
    pub length: usize,
    pub dtor: Option<unsafe extern "C" fn(*mut char16_t)>,
}

impl Default for cef_string_utf16_t {
    fn default() -> Self {
        Self {
            str: ptr::null_mut(),
            length: 0,
            dtor: None,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct cef_string_utf8_t {
    pub str: *mut c_char,
    pub length: usize,
    pub dtor: Option<unsafe extern "C" fn(*mut c_char)>,
}

impl Default for cef_string_utf8_t {
    fn default() -> Self {
        Self {
            str: ptr::null_mut(),
            length: 0,
            dtor: None,
        }
    }
}

pub type cef_string_t = cef_string_utf16_t;
pub type cef_string_userfree_t = *mut cef_string_t;
pub type cef_string_list_t = *mut c_void;
pub type cef_string_map_t = *mut c_void;
pub type cef_string_multimap_t = *mut c_void;

pub type cef_color_t = u32;
pub type cef_basetime_t = i64;
pub type cef_errorcode_t = c_int;
pub type cef_transition_type_t = u32;
pub type cef_value_type_t = c_int;
pub type cef_log_items_t = c_int;
pub type cef_window_handle_t = *mut c_void;
pub type cef_cursor_handle_t = *mut c_void;
pub type cef_cursor_type_t = c_int;
pub type cef_referrer_policy_t = c_int;
pub type cef_resource_type_t = c_int;
pub type cef_urlrequest_status_t = c_int;
pub type cef_termination_status_t = c_int;
pub type cef_preferences_type_t = c_int;
pub type cef_content_setting_types_t = c_int;
pub type cef_content_setting_values_t = c_int;
pub type cef_color_variant_t = c_int;

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cef_process_id_t {
    PID_BROWSER = 0,
    PID_RENDERER = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cef_state_t {
    STATE_DEFAULT = 0,
    STATE_ENABLED = 1,
    STATE_DISABLED = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cef_return_value_t {
    RV_CANCEL = 0,
    RV_CONTINUE = 1,
    RV_CONTINUE_ASYNC = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cef_window_open_disposition_t {
    CEF_WOD_UNKNOWN = 0,
    CEF_WOD_CURRENT_TAB = 1,
    CEF_WOD_SINGLETON_TAB = 2,
    CEF_WOD_NEW_FOREGROUND_TAB = 3,
    CEF_WOD_NEW_BACKGROUND_TAB = 4,
    CEF_WOD_NEW_POPUP = 5,
    CEF_WOD_NEW_WINDOW = 6,
    CEF_WOD_SAVE_TO_DISK = 7,
    CEF_WOD_OFF_THE_RECORD = 8,
    CEF_WOD_IGNORE_ACTION = 9,
    CEF_WOD_SWITCH_TO_TAB = 10,
    CEF_WOD_NEW_PICTURE_IN_PICTURE = 11,
    CEF_WOD_NEW_SPLIT_VIEW = 12,
    CEF_WOD_NUM_VALUES = 13,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cef_runtime_style_t {
    CEF_RUNTIME_STYLE_DEFAULT = 0,
    CEF_RUNTIME_STYLE_CHROME = 1,
    CEF_RUNTIME_STYLE_ALLOY = 2,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cef_log_severity_t {
    LOGSEVERITY_DEFAULT = 0,
    LOGSEVERITY_VERBOSE = 1,
    LOGSEVERITY_INFO = 2,
    LOGSEVERITY_WARNING = 3,
    LOGSEVERITY_ERROR = 4,
    LOGSEVERITY_FATAL = 5,
    LOGSEVERITY_DISABLE = 99,
}

pub const CEF_SCHEME_OPTION_NONE: c_int = 0;
pub const CEF_SCHEME_OPTION_STANDARD: c_int = 1 << 0;
pub const CEF_SCHEME_OPTION_LOCAL: c_int = 1 << 1;
pub const CEF_SCHEME_OPTION_DISPLAY_ISOLATED: c_int = 1 << 2;
pub const CEF_SCHEME_OPTION_SECURE: c_int = 1 << 3;
pub const CEF_SCHEME_OPTION_CORS_ENABLED: c_int = 1 << 4;
pub const CEF_SCHEME_OPTION_CSP_BYPASSING: c_int = 1 << 5;
pub const CEF_SCHEME_OPTION_FETCH_ENABLED: c_int = 1 << 6;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_rect_t {
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_point_t {
    pub x: c_int,
    pub y: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_size_t {
    pub width: c_int,
    pub height: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct cef_base_ref_counted_t {
    pub size: usize,
    pub add_ref: Option<unsafe extern "C" fn(*mut cef_base_ref_counted_t)>,
    pub release: Option<unsafe extern "C" fn(*mut cef_base_ref_counted_t) -> c_int>,
    pub has_one_ref: Option<unsafe extern "C" fn(*mut cef_base_ref_counted_t) -> c_int>,
    pub has_at_least_one_ref: Option<unsafe extern "C" fn(*mut cef_base_ref_counted_t) -> c_int>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct cef_base_scoped_t {
    pub size: usize,
    pub del: Option<unsafe extern "C" fn(*mut cef_base_scoped_t)>,
}

#[cfg(target_os = "macos")]
#[repr(C)]
pub struct cef_main_args_t {
    pub argc: c_int,
    pub argv: *mut *mut c_char,
}

#[cfg(target_os = "linux")]
#[repr(C)]
pub struct cef_main_args_t {
    pub argc: c_int,
    pub argv: *mut *mut c_char,
}

#[cfg(target_os = "windows")]
#[repr(C)]
pub struct cef_main_args_t {
    pub instance: *mut c_void,
}

#[cfg(target_os = "macos")]
#[repr(C)]
pub struct cef_window_info_t {
    pub size: usize,
    pub window_name: cef_string_t,
    pub bounds: cef_rect_t,
    pub hidden: c_int,
    pub parent_view: cef_window_handle_t,
    pub windowless_rendering_enabled: c_int,
    pub shared_texture_enabled: c_int,
    pub external_begin_frame_enabled: c_int,
    pub view: cef_window_handle_t,
    pub runtime_style: cef_runtime_style_t,
}

#[cfg(target_os = "macos")]
impl Default for cef_window_info_t {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            window_name: cef_string_t::default(),
            bounds: cef_rect_t::default(),
            hidden: 0,
            parent_view: ptr::null_mut(),
            windowless_rendering_enabled: 0,
            shared_texture_enabled: 0,
            external_begin_frame_enabled: 0,
            view: ptr::null_mut(),
            runtime_style: cef_runtime_style_t::CEF_RUNTIME_STYLE_DEFAULT,
        }
    }
}

#[cfg(target_os = "windows")]
#[repr(C)]
pub struct cef_window_info_t {
    pub size: usize,
    pub ex_style: u32,
    pub window_name: cef_string_t,
    pub style: u32,
    pub bounds: cef_rect_t,
    pub parent_window: cef_window_handle_t,
    pub menu: *mut c_void,
    pub windowless_rendering_enabled: c_int,
    pub shared_texture_enabled: c_int,
    pub external_begin_frame_enabled: c_int,
    pub window: cef_window_handle_t,
    pub runtime_style: cef_runtime_style_t,
}

#[cfg(target_os = "windows")]
impl Default for cef_window_info_t {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            ex_style: 0,
            window_name: cef_string_t::default(),
            style: 0,
            bounds: cef_rect_t::default(),
            parent_window: ptr::null_mut(),
            menu: ptr::null_mut(),
            windowless_rendering_enabled: 0,
            shared_texture_enabled: 0,
            external_begin_frame_enabled: 0,
            window: ptr::null_mut(),
            runtime_style: cef_runtime_style_t::CEF_RUNTIME_STYLE_DEFAULT,
        }
    }
}

#[cfg(target_os = "linux")]
#[repr(C)]
pub struct cef_window_info_t {
    pub size: usize,
    pub window_name: cef_string_t,
    pub bounds: cef_rect_t,
    pub parent_window: cef_window_handle_t,
    pub windowless_rendering_enabled: c_int,
    pub shared_texture_enabled: c_int,
    pub external_begin_frame_enabled: c_int,
    pub window: cef_window_handle_t,
    pub runtime_style: cef_runtime_style_t,
}

#[cfg(target_os = "linux")]
impl Default for cef_window_info_t {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            window_name: cef_string_t::default(),
            bounds: cef_rect_t::default(),
            parent_window: ptr::null_mut(),
            windowless_rendering_enabled: 0,
            shared_texture_enabled: 0,
            external_begin_frame_enabled: 0,
            window: ptr::null_mut(),
            runtime_style: cef_runtime_style_t::CEF_RUNTIME_STYLE_DEFAULT,
        }
    }
}

#[repr(C)]
pub struct cef_settings_t {
    pub size: usize,
    pub no_sandbox: c_int,
    pub browser_subprocess_path: cef_string_t,
    pub framework_dir_path: cef_string_t,
    pub main_bundle_path: cef_string_t,
    pub multi_threaded_message_loop: c_int,
    pub external_message_pump: c_int,
    pub windowless_rendering_enabled: c_int,
    pub command_line_args_disabled: c_int,
    pub cache_path: cef_string_t,
    pub root_cache_path: cef_string_t,
    pub persist_session_cookies: c_int,
    pub user_agent: cef_string_t,
    pub user_agent_product: cef_string_t,
    pub locale: cef_string_t,
    pub log_file: cef_string_t,
    pub log_severity: cef_log_severity_t,
    pub log_items: cef_log_items_t,
    pub javascript_flags: cef_string_t,
    pub resources_dir_path: cef_string_t,
    pub locales_dir_path: cef_string_t,
    pub remote_debugging_port: c_int,
    pub uncaught_exception_stack_size: c_int,
    pub background_color: cef_color_t,
    pub accept_language_list: cef_string_t,
    pub cookieable_schemes_list: cef_string_t,
    pub cookieable_schemes_exclude_defaults: c_int,
    pub chrome_policy_id: cef_string_t,
    pub chrome_app_icon_id: c_int,
    pub disable_signal_handlers: c_int,
    pub use_views_default_popup: c_int,
    pub keychain_service_name: cef_string_t,
    pub keychain_account_name: cef_string_t,
}

impl Default for cef_settings_t {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            no_sandbox: 0,
            browser_subprocess_path: cef_string_t::default(),
            framework_dir_path: cef_string_t::default(),
            main_bundle_path: cef_string_t::default(),
            multi_threaded_message_loop: 0,
            external_message_pump: 0,
            windowless_rendering_enabled: 0,
            command_line_args_disabled: 0,
            cache_path: cef_string_t::default(),
            root_cache_path: cef_string_t::default(),
            persist_session_cookies: 0,
            user_agent: cef_string_t::default(),
            user_agent_product: cef_string_t::default(),
            locale: cef_string_t::default(),
            log_file: cef_string_t::default(),
            log_severity: cef_log_severity_t::LOGSEVERITY_DEFAULT,
            log_items: 0,
            javascript_flags: cef_string_t::default(),
            resources_dir_path: cef_string_t::default(),
            locales_dir_path: cef_string_t::default(),
            remote_debugging_port: 0,
            uncaught_exception_stack_size: 0,
            background_color: 0,
            accept_language_list: cef_string_t::default(),
            cookieable_schemes_list: cef_string_t::default(),
            cookieable_schemes_exclude_defaults: 0,
            chrome_policy_id: cef_string_t::default(),
            chrome_app_icon_id: 0,
            disable_signal_handlers: 0,
            use_views_default_popup: 0,
            keychain_service_name: cef_string_t::default(),
            keychain_account_name: cef_string_t::default(),
        }
    }
}

#[repr(C)]
pub struct cef_browser_settings_t {
    pub size: usize,
    pub windowless_frame_rate: c_int,
    pub standard_font_family: cef_string_t,
    pub fixed_font_family: cef_string_t,
    pub serif_font_family: cef_string_t,
    pub sans_serif_font_family: cef_string_t,
    pub cursive_font_family: cef_string_t,
    pub fantasy_font_family: cef_string_t,
    pub default_font_size: c_int,
    pub default_fixed_font_size: c_int,
    pub minimum_font_size: c_int,
    pub minimum_logical_font_size: c_int,
    pub default_encoding: cef_string_t,
    pub remote_fonts: cef_state_t,
    pub javascript: cef_state_t,
    pub javascript_close_windows: cef_state_t,
    pub javascript_access_clipboard: cef_state_t,
    pub javascript_dom_paste: cef_state_t,
    pub image_loading: cef_state_t,
    pub image_shrink_standalone_to_fit: cef_state_t,
    pub text_area_resize: cef_state_t,
    pub tab_to_links: cef_state_t,
    pub local_storage: cef_state_t,
    pub databases_deprecated: cef_state_t,
    pub webgl: cef_state_t,
    pub background_color: cef_color_t,
    pub chrome_status_bubble: cef_state_t,
    pub chrome_zoom_bubble: cef_state_t,
}

impl Default for cef_browser_settings_t {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            windowless_frame_rate: 0,
            standard_font_family: cef_string_t::default(),
            fixed_font_family: cef_string_t::default(),
            serif_font_family: cef_string_t::default(),
            sans_serif_font_family: cef_string_t::default(),
            cursive_font_family: cef_string_t::default(),
            fantasy_font_family: cef_string_t::default(),
            default_font_size: 0,
            default_fixed_font_size: 0,
            minimum_font_size: 0,
            minimum_logical_font_size: 0,
            default_encoding: cef_string_t::default(),
            remote_fonts: cef_state_t::STATE_DEFAULT,
            javascript: cef_state_t::STATE_DEFAULT,
            javascript_close_windows: cef_state_t::STATE_DEFAULT,
            javascript_access_clipboard: cef_state_t::STATE_DEFAULT,
            javascript_dom_paste: cef_state_t::STATE_DEFAULT,
            image_loading: cef_state_t::STATE_DEFAULT,
            image_shrink_standalone_to_fit: cef_state_t::STATE_DEFAULT,
            text_area_resize: cef_state_t::STATE_DEFAULT,
            tab_to_links: cef_state_t::STATE_DEFAULT,
            local_storage: cef_state_t::STATE_DEFAULT,
            databases_deprecated: cef_state_t::STATE_DEFAULT,
            webgl: cef_state_t::STATE_DEFAULT,
            background_color: 0,
            chrome_status_bubble: cef_state_t::STATE_DEFAULT,
            chrome_zoom_bubble: cef_state_t::STATE_DEFAULT,
        }
    }
}

#[repr(C)]
pub struct cef_request_context_settings_t {
    pub size: usize,
    pub cache_path: cef_string_t,
    pub persist_session_cookies: c_int,
    pub accept_language_list: cef_string_t,
    pub cookieable_schemes_list: cef_string_t,
    pub cookieable_schemes_exclude_defaults: c_int,
}

impl Default for cef_request_context_settings_t {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            cache_path: cef_string_t::default(),
            persist_session_cookies: 0,
            accept_language_list: cef_string_t::default(),
            cookieable_schemes_list: cef_string_t::default(),
            cookieable_schemes_exclude_defaults: 0,
        }
    }
}

#[repr(C)]
pub struct cef_scheme_registrar_t {
    pub base: cef_base_scoped_t,
    pub add_custom_scheme: Option<
        unsafe extern "C" fn(*mut cef_scheme_registrar_t, *const cef_string_t, c_int) -> c_int,
    >,
}

#[repr(C)]
pub struct cef_command_line_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "C" fn(*mut cef_command_line_t) -> c_int>,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_command_line_t) -> c_int>,
    pub copy: Option<unsafe extern "C" fn(*mut cef_command_line_t) -> *mut cef_command_line_t>,
    pub init_from_argv:
        Option<unsafe extern "C" fn(*mut cef_command_line_t, c_int, *const *const c_char)>,
    pub init_from_string:
        Option<unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t)>,
    pub reset: Option<unsafe extern "C" fn(*mut cef_command_line_t)>,
    pub get_argv: Option<unsafe extern "C" fn(*mut cef_command_line_t, cef_string_list_t)>,
    pub get_command_line_string:
        Option<unsafe extern "C" fn(*mut cef_command_line_t) -> cef_string_userfree_t>,
    pub get_program: Option<unsafe extern "C" fn(*mut cef_command_line_t) -> cef_string_userfree_t>,
    pub set_program: Option<unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t)>,
    pub has_switches: Option<unsafe extern "C" fn(*mut cef_command_line_t) -> c_int>,
    pub has_switch:
        Option<unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t) -> c_int>,
    pub get_switch_value: Option<
        unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t) -> cef_string_userfree_t,
    >,
    pub get_switches: Option<unsafe extern "C" fn(*mut cef_command_line_t, cef_string_map_t)>,
    pub append_switch: Option<unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t)>,
    pub append_switch_with_value: Option<
        unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t, *const cef_string_t),
    >,
    pub has_arguments: Option<unsafe extern "C" fn(*mut cef_command_line_t) -> c_int>,
    pub get_arguments: Option<unsafe extern "C" fn(*mut cef_command_line_t, cef_string_list_t)>,
    pub append_argument: Option<unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t)>,
    pub prepend_wrapper: Option<unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t)>,
    pub remove_switch: Option<unsafe extern "C" fn(*mut cef_command_line_t, *const cef_string_t)>,
}

#[repr(C)]
pub struct cef_app_t {
    pub base: cef_base_ref_counted_t,
    pub on_before_command_line_processing:
        Option<unsafe extern "C" fn(*mut cef_app_t, *const cef_string_t, *mut cef_command_line_t)>,
    pub on_register_custom_schemes:
        Option<unsafe extern "C" fn(*mut cef_app_t, *mut cef_scheme_registrar_t)>,
    pub get_resource_bundle_handler: Option<unsafe extern "C" fn(*mut cef_app_t) -> *mut c_void>,
    pub get_browser_process_handler:
        Option<unsafe extern "C" fn(*mut cef_app_t) -> *mut cef_browser_process_handler_t>,
    pub get_render_process_handler: Option<unsafe extern "C" fn(*mut cef_app_t) -> *mut c_void>,
}

#[repr(C)]
pub struct cef_browser_process_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_register_custom_preferences: Option<
        unsafe extern "C" fn(
            *mut cef_browser_process_handler_t,
            cef_preferences_type_t,
            *mut c_void,
        ),
    >,
    pub on_context_initialized: Option<unsafe extern "C" fn(*mut cef_browser_process_handler_t)>,
    pub on_before_child_process_launch:
        Option<unsafe extern "C" fn(*mut cef_browser_process_handler_t, *mut cef_command_line_t)>,
    pub on_already_running_app_relaunch: Option<
        unsafe extern "C" fn(
            *mut cef_browser_process_handler_t,
            *mut cef_command_line_t,
            *const cef_string_t,
        ) -> c_int,
    >,
    pub on_schedule_message_pump_work:
        Option<unsafe extern "C" fn(*mut cef_browser_process_handler_t, i64)>,
    pub get_default_client:
        Option<unsafe extern "C" fn(*mut cef_browser_process_handler_t) -> *mut cef_client_t>,
    pub get_default_request_context_handler:
        Option<unsafe extern "C" fn(*mut cef_browser_process_handler_t) -> *mut c_void>,
}

#[repr(C)]
pub struct cef_client_t {
    pub base: cef_base_ref_counted_t,
    pub get_audio_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_command_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_context_menu_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_dialog_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_display_handler:
        Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut cef_display_handler_t>,
    pub get_download_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_drag_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_find_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_focus_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_frame_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_permission_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_jsdialog_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_keyboard_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_life_span_handler:
        Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut cef_life_span_handler_t>,
    pub get_load_handler:
        Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut cef_load_handler_t>,
    pub get_print_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_render_handler: Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut c_void>,
    pub get_request_handler:
        Option<unsafe extern "C" fn(*mut cef_client_t) -> *mut cef_request_handler_t>,
    pub on_process_message_received: Option<
        unsafe extern "C" fn(
            *mut cef_client_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            cef_process_id_t,
            *mut cef_process_message_t,
        ) -> c_int,
    >,
}

#[repr(C)]
pub struct cef_find_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_find_result: Option<
        unsafe extern "C" fn(
            *mut cef_find_handler_t,
            *mut cef_browser_t,
            c_int,
            c_int,
            *const c_void,
            c_int,
            c_int,
        ),
    >,
}

#[repr(C)]
pub struct cef_drag_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_drag_enter: Option<
        unsafe extern "C" fn(
            *mut cef_drag_handler_t,
            *mut cef_browser_t,
            *mut cef_drag_data_t,
            u32,
        ) -> c_int,
    >,
    pub on_draggable_regions_changed: Option<
        unsafe extern "C" fn(
            *mut cef_drag_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            usize,
            *const c_void,
        ),
    >,
}

type DragStr = Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> cef_string_userfree_t>;
type DragSet = Option<unsafe extern "C" fn(*mut cef_drag_data_t, *const cef_string_t)>;

#[repr(C)]
pub struct cef_drag_data_t {
    pub base: cef_base_ref_counted_t,
    pub clone: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> *mut cef_drag_data_t>,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> c_int>,
    pub is_link: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> c_int>,
    pub is_fragment: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> c_int>,
    pub is_file: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> c_int>,
    pub get_link_url: DragStr,
    pub get_link_title: DragStr,
    pub get_link_metadata: DragStr,
    pub get_fragment_text: DragStr,
    pub get_fragment_html: DragStr,
    pub get_fragment_base_url: DragStr,
    pub get_file_name: DragStr,
    pub get_file_contents: Option<unsafe extern "C" fn(*mut cef_drag_data_t, *mut c_void) -> usize>,
    pub get_file_names:
        Option<unsafe extern "C" fn(*mut cef_drag_data_t, cef_string_list_t) -> c_int>,
    pub get_file_paths:
        Option<unsafe extern "C" fn(*mut cef_drag_data_t, cef_string_list_t) -> c_int>,
    pub set_link_url: DragSet,
    pub set_link_title: DragSet,
    pub set_link_metadata: DragSet,
    pub set_fragment_text: DragSet,
    pub set_fragment_html: DragSet,
    pub set_fragment_base_url: DragSet,
    pub reset_file_contents: Option<unsafe extern "C" fn(*mut cef_drag_data_t)>,
    pub add_file: Option<
        unsafe extern "C" fn(*mut cef_drag_data_t, *const cef_string_t, *const cef_string_t),
    >,
    pub clear_filenames: Option<unsafe extern "C" fn(*mut cef_drag_data_t)>,
    pub get_image: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> *mut c_void>,
    pub get_image_hotspot: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> cef_point_t>,
    pub has_image: Option<unsafe extern "C" fn(*mut cef_drag_data_t) -> c_int>,
}

#[repr(C)]
pub struct cef_life_span_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_before_popup: Option<
        unsafe extern "C" fn(
            *mut cef_life_span_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            c_int,
            *const cef_string_t,
            *const cef_string_t,
            cef_window_open_disposition_t,
            c_int,
            *const c_void,
            *mut cef_window_info_t,
            *mut *mut cef_client_t,
            *mut cef_browser_settings_t,
            *mut *mut cef_dictionary_value_t,
            *mut c_int,
        ) -> c_int,
    >,
    pub on_before_popup_aborted:
        Option<unsafe extern "C" fn(*mut cef_life_span_handler_t, *mut cef_browser_t, c_int)>,
    pub on_before_dev_tools_popup: Option<
        unsafe extern "C" fn(
            *mut cef_life_span_handler_t,
            *mut cef_browser_t,
            *mut cef_window_info_t,
            *mut *mut cef_client_t,
            *mut cef_browser_settings_t,
            *mut *mut cef_dictionary_value_t,
            *mut c_int,
        ),
    >,
    pub on_after_created:
        Option<unsafe extern "C" fn(*mut cef_life_span_handler_t, *mut cef_browser_t)>,
    pub do_close:
        Option<unsafe extern "C" fn(*mut cef_life_span_handler_t, *mut cef_browser_t) -> c_int>,
    pub on_before_close:
        Option<unsafe extern "C" fn(*mut cef_life_span_handler_t, *mut cef_browser_t)>,
}

#[repr(C)]
pub struct cef_load_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_loading_state_change: Option<
        unsafe extern "C" fn(*mut cef_load_handler_t, *mut cef_browser_t, c_int, c_int, c_int),
    >,
    pub on_load_start: Option<
        unsafe extern "C" fn(
            *mut cef_load_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            cef_transition_type_t,
        ),
    >,
    pub on_load_end: Option<
        unsafe extern "C" fn(*mut cef_load_handler_t, *mut cef_browser_t, *mut cef_frame_t, c_int),
    >,
    pub on_load_error: Option<
        unsafe extern "C" fn(
            *mut cef_load_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            cef_errorcode_t,
            *const cef_string_t,
            *const cef_string_t,
        ),
    >,
}

#[repr(C)]
pub struct cef_display_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_address_change: Option<
        unsafe extern "C" fn(
            *mut cef_display_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *const cef_string_t,
        ),
    >,
    pub on_title_change: Option<
        unsafe extern "C" fn(*mut cef_display_handler_t, *mut cef_browser_t, *const cef_string_t),
    >,
    pub on_favicon_urlchange: Option<
        unsafe extern "C" fn(*mut cef_display_handler_t, *mut cef_browser_t, cef_string_list_t),
    >,
    pub on_fullscreen_mode_change:
        Option<unsafe extern "C" fn(*mut cef_display_handler_t, *mut cef_browser_t, c_int)>,
    pub on_tooltip: Option<
        unsafe extern "C" fn(
            *mut cef_display_handler_t,
            *mut cef_browser_t,
            *mut cef_string_t,
        ) -> c_int,
    >,
    pub on_status_message: Option<
        unsafe extern "C" fn(*mut cef_display_handler_t, *mut cef_browser_t, *const cef_string_t),
    >,
    pub on_console_message: Option<
        unsafe extern "C" fn(
            *mut cef_display_handler_t,
            *mut cef_browser_t,
            cef_log_severity_t,
            *const cef_string_t,
            *const cef_string_t,
            c_int,
        ) -> c_int,
    >,
    pub on_auto_resize: Option<
        unsafe extern "C" fn(
            *mut cef_display_handler_t,
            *mut cef_browser_t,
            *const cef_size_t,
        ) -> c_int,
    >,
    pub on_loading_progress_change:
        Option<unsafe extern "C" fn(*mut cef_display_handler_t, *mut cef_browser_t, f64)>,
    pub on_cursor_change: Option<
        unsafe extern "C" fn(
            *mut cef_display_handler_t,
            *mut cef_browser_t,
            cef_cursor_handle_t,
            cef_cursor_type_t,
            *const c_void,
        ) -> c_int,
    >,
    pub on_media_access_change:
        Option<unsafe extern "C" fn(*mut cef_display_handler_t, *mut cef_browser_t, c_int, c_int)>,
    pub on_contents_bounds_change: Option<
        unsafe extern "C" fn(
            *mut cef_display_handler_t,
            *mut cef_browser_t,
            *const cef_rect_t,
        ) -> c_int,
    >,
    pub get_root_window_screen_rect: Option<
        unsafe extern "C" fn(
            *mut cef_display_handler_t,
            *mut cef_browser_t,
            *mut cef_rect_t,
        ) -> c_int,
    >,
}

#[repr(C)]
pub struct cef_request_handler_t {
    pub base: cef_base_ref_counted_t,
    pub on_before_browse: Option<
        unsafe extern "C" fn(
            *mut cef_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            c_int,
            c_int,
        ) -> c_int,
    >,
    pub on_open_urlfrom_tab: Option<
        unsafe extern "C" fn(
            *mut cef_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *const cef_string_t,
            cef_window_open_disposition_t,
            c_int,
        ) -> c_int,
    >,
    pub get_resource_request_handler: Option<
        unsafe extern "C" fn(
            *mut cef_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            c_int,
            c_int,
            *const cef_string_t,
            *mut c_int,
        ) -> *mut cef_resource_request_handler_t,
    >,
    pub get_auth_credentials: Option<
        unsafe extern "C" fn(
            *mut cef_request_handler_t,
            *mut cef_browser_t,
            *const cef_string_t,
            c_int,
            *const cef_string_t,
            c_int,
            *const cef_string_t,
            *const cef_string_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub on_certificate_error: Option<
        unsafe extern "C" fn(
            *mut cef_request_handler_t,
            *mut cef_browser_t,
            cef_errorcode_t,
            *const cef_string_t,
            *mut c_void,
            *mut cef_callback_t,
        ) -> c_int,
    >,
    pub on_select_client_certificate: Option<
        unsafe extern "C" fn(
            *mut cef_request_handler_t,
            *mut cef_browser_t,
            c_int,
            *const cef_string_t,
            c_int,
            usize,
            *const *mut c_void,
            *mut c_void,
        ) -> c_int,
    >,
    pub on_render_view_ready:
        Option<unsafe extern "C" fn(*mut cef_request_handler_t, *mut cef_browser_t)>,
    pub on_render_process_unresponsive: Option<
        unsafe extern "C" fn(*mut cef_request_handler_t, *mut cef_browser_t, *mut c_void) -> c_int,
    >,
    pub on_render_process_responsive:
        Option<unsafe extern "C" fn(*mut cef_request_handler_t, *mut cef_browser_t)>,
    pub on_render_process_terminated: Option<
        unsafe extern "C" fn(
            *mut cef_request_handler_t,
            *mut cef_browser_t,
            cef_termination_status_t,
            c_int,
            *const cef_string_t,
        ),
    >,
    pub on_document_available_in_main_frame:
        Option<unsafe extern "C" fn(*mut cef_request_handler_t, *mut cef_browser_t)>,
}

#[repr(C)]
pub struct cef_resource_request_handler_t {
    pub base: cef_base_ref_counted_t,
    pub get_cookie_access_filter: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
        ) -> *mut c_void,
    >,
    pub on_before_resource_load: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            *mut cef_callback_t,
        ) -> cef_return_value_t,
    >,
    pub get_resource_handler: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
        ) -> *mut cef_resource_handler_t,
    >,
    pub on_resource_redirect: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            *mut cef_response_t,
            *mut cef_string_t,
        ),
    >,
    pub on_resource_response: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            *mut cef_response_t,
        ) -> c_int,
    >,
    pub get_resource_response_filter: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            *mut cef_response_t,
        ) -> *mut c_void,
    >,
    pub on_resource_load_complete: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            *mut cef_response_t,
            cef_urlrequest_status_t,
            i64,
        ),
    >,
    pub on_protocol_execution: Option<
        unsafe extern "C" fn(
            *mut cef_resource_request_handler_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *mut cef_request_t,
            *mut c_int,
        ),
    >,
}

#[repr(C)]
pub struct cef_scheme_handler_factory_t {
    pub base: cef_base_ref_counted_t,
    pub create: Option<
        unsafe extern "C" fn(
            *mut cef_scheme_handler_factory_t,
            *mut cef_browser_t,
            *mut cef_frame_t,
            *const cef_string_t,
            *mut cef_request_t,
        ) -> *mut cef_resource_handler_t,
    >,
}

#[repr(C)]
pub struct cef_resource_handler_t {
    pub base: cef_base_ref_counted_t,
    pub open: Option<
        unsafe extern "C" fn(
            *mut cef_resource_handler_t,
            *mut cef_request_t,
            *mut c_int,
            *mut cef_callback_t,
        ) -> c_int,
    >,
    pub process_request: Option<
        unsafe extern "C" fn(
            *mut cef_resource_handler_t,
            *mut cef_request_t,
            *mut cef_callback_t,
        ) -> c_int,
    >,
    pub get_response_headers: Option<
        unsafe extern "C" fn(
            *mut cef_resource_handler_t,
            *mut cef_response_t,
            *mut i64,
            *mut cef_string_t,
        ),
    >,
    pub skip: Option<
        unsafe extern "C" fn(*mut cef_resource_handler_t, i64, *mut i64, *mut c_void) -> c_int,
    >,
    pub read: Option<
        unsafe extern "C" fn(
            *mut cef_resource_handler_t,
            *mut c_void,
            c_int,
            *mut c_int,
            *mut c_void,
        ) -> c_int,
    >,
    pub read_response: Option<
        unsafe extern "C" fn(
            *mut cef_resource_handler_t,
            *mut c_void,
            c_int,
            *mut c_int,
            *mut cef_callback_t,
        ) -> c_int,
    >,
    pub cancel: Option<unsafe extern "C" fn(*mut cef_resource_handler_t)>,
}

#[repr(C)]
pub struct cef_callback_t {
    pub base: cef_base_ref_counted_t,
    pub cont: Option<unsafe extern "C" fn(*mut cef_callback_t)>,
    pub cancel: Option<unsafe extern "C" fn(*mut cef_callback_t)>,
}

#[repr(C)]
pub struct cef_completion_callback_t {
    pub base: cef_base_ref_counted_t,
    pub on_complete: Option<unsafe extern "C" fn(*mut cef_completion_callback_t)>,
}

#[repr(C)]
pub struct cef_cookie_manager_t {
    pub base: cef_base_ref_counted_t,
    pub visit_all_cookies:
        Option<unsafe extern "C" fn(*mut cef_cookie_manager_t, *mut c_void) -> c_int>,
    pub visit_url_cookies: Option<
        unsafe extern "C" fn(
            *mut cef_cookie_manager_t,
            *const cef_string_t,
            c_int,
            *mut c_void,
        ) -> c_int,
    >,
    pub set_cookie: Option<
        unsafe extern "C" fn(
            *mut cef_cookie_manager_t,
            *const cef_string_t,
            *const c_void,
            *mut c_void,
        ) -> c_int,
    >,
    pub delete_cookies: Option<
        unsafe extern "C" fn(
            *mut cef_cookie_manager_t,
            *const cef_string_t,
            *const cef_string_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub flush_store: Option<
        unsafe extern "C" fn(*mut cef_cookie_manager_t, *mut cef_completion_callback_t) -> c_int,
    >,
}

#[repr(C)]
pub struct cef_preference_manager_t {
    pub base: cef_base_ref_counted_t,
    pub has_preference:
        Option<unsafe extern "C" fn(*mut cef_preference_manager_t, *const cef_string_t) -> c_int>,
    pub get_preference: Option<
        unsafe extern "C" fn(*mut cef_preference_manager_t, *const cef_string_t) -> *mut c_void,
    >,
    pub get_all_preferences: Option<
        unsafe extern "C" fn(*mut cef_preference_manager_t, c_int) -> *mut cef_dictionary_value_t,
    >,
    pub can_set_preference:
        Option<unsafe extern "C" fn(*mut cef_preference_manager_t, *const cef_string_t) -> c_int>,
    pub set_preference: Option<
        unsafe extern "C" fn(
            *mut cef_preference_manager_t,
            *const cef_string_t,
            *mut c_void,
            *mut cef_string_t,
        ) -> c_int,
    >,
    pub add_preference_observer: Option<
        unsafe extern "C" fn(
            *mut cef_preference_manager_t,
            *const cef_string_t,
            *mut c_void,
        ) -> *mut c_void,
    >,
}

#[repr(C)]
pub struct cef_request_context_t {
    pub base: cef_preference_manager_t,
    pub is_same: Option<
        unsafe extern "C" fn(*mut cef_request_context_t, *mut cef_request_context_t) -> c_int,
    >,
    pub is_sharing_with: Option<
        unsafe extern "C" fn(*mut cef_request_context_t, *mut cef_request_context_t) -> c_int,
    >,
    pub is_global: Option<unsafe extern "C" fn(*mut cef_request_context_t) -> c_int>,
    pub get_handler: Option<unsafe extern "C" fn(*mut cef_request_context_t) -> *mut c_void>,
    pub get_cache_path:
        Option<unsafe extern "C" fn(*mut cef_request_context_t) -> cef_string_userfree_t>,
    pub get_cookie_manager: Option<
        unsafe extern "C" fn(
            *mut cef_request_context_t,
            *mut cef_completion_callback_t,
        ) -> *mut cef_cookie_manager_t,
    >,
    pub register_scheme_handler_factory: Option<
        unsafe extern "C" fn(
            *mut cef_request_context_t,
            *const cef_string_t,
            *const cef_string_t,
            *mut cef_scheme_handler_factory_t,
        ) -> c_int,
    >,
    pub clear_scheme_handler_factories:
        Option<unsafe extern "C" fn(*mut cef_request_context_t) -> c_int>,
    pub clear_certificate_exceptions:
        Option<unsafe extern "C" fn(*mut cef_request_context_t, *mut cef_completion_callback_t)>,
    pub clear_http_auth_credentials:
        Option<unsafe extern "C" fn(*mut cef_request_context_t, *mut cef_completion_callback_t)>,
    pub close_all_connections:
        Option<unsafe extern "C" fn(*mut cef_request_context_t, *mut cef_completion_callback_t)>,
    pub resolve_host:
        Option<unsafe extern "C" fn(*mut cef_request_context_t, *const cef_string_t, *mut c_void)>,
    pub get_media_router: Option<
        unsafe extern "C" fn(
            *mut cef_request_context_t,
            *mut cef_completion_callback_t,
        ) -> *mut c_void,
    >,
    pub get_website_setting: Option<
        unsafe extern "C" fn(
            *mut cef_request_context_t,
            *const cef_string_t,
            *const cef_string_t,
            cef_content_setting_types_t,
        ) -> *mut c_void,
    >,
    pub set_website_setting: Option<
        unsafe extern "C" fn(
            *mut cef_request_context_t,
            *const cef_string_t,
            *const cef_string_t,
            cef_content_setting_types_t,
            *mut c_void,
        ),
    >,
    pub get_content_setting: Option<
        unsafe extern "C" fn(
            *mut cef_request_context_t,
            *const cef_string_t,
            *const cef_string_t,
            cef_content_setting_types_t,
        ) -> cef_content_setting_values_t,
    >,
    pub set_content_setting: Option<
        unsafe extern "C" fn(
            *mut cef_request_context_t,
            *const cef_string_t,
            *const cef_string_t,
            cef_content_setting_types_t,
            cef_content_setting_values_t,
        ),
    >,
    pub set_chrome_color_scheme:
        Option<unsafe extern "C" fn(*mut cef_request_context_t, cef_color_variant_t, cef_color_t)>,
    pub get_chrome_color_scheme_mode:
        Option<unsafe extern "C" fn(*mut cef_request_context_t) -> cef_color_variant_t>,
    pub get_chrome_color_scheme_color:
        Option<unsafe extern "C" fn(*mut cef_request_context_t) -> cef_color_t>,
    pub get_chrome_color_scheme_variant:
        Option<unsafe extern "C" fn(*mut cef_request_context_t) -> cef_color_variant_t>,
    pub add_setting_observer:
        Option<unsafe extern "C" fn(*mut cef_request_context_t, *mut c_void) -> *mut c_void>,
    pub clear_http_cache:
        Option<unsafe extern "C" fn(*mut cef_request_context_t, *mut cef_completion_callback_t)>,
}

#[repr(C)]
pub struct cef_browser_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "C" fn(*mut cef_browser_t) -> c_int>,
    pub get_host: Option<unsafe extern "C" fn(*mut cef_browser_t) -> *mut cef_browser_host_t>,
    pub can_go_back: Option<unsafe extern "C" fn(*mut cef_browser_t) -> c_int>,
    pub go_back: Option<unsafe extern "C" fn(*mut cef_browser_t)>,
    pub can_go_forward: Option<unsafe extern "C" fn(*mut cef_browser_t) -> c_int>,
    pub go_forward: Option<unsafe extern "C" fn(*mut cef_browser_t)>,
    pub is_loading: Option<unsafe extern "C" fn(*mut cef_browser_t) -> c_int>,
    pub reload: Option<unsafe extern "C" fn(*mut cef_browser_t)>,
    pub reload_ignore_cache: Option<unsafe extern "C" fn(*mut cef_browser_t)>,
    pub stop_load: Option<unsafe extern "C" fn(*mut cef_browser_t)>,
    pub get_identifier: Option<unsafe extern "C" fn(*mut cef_browser_t) -> c_int>,
    pub is_same: Option<unsafe extern "C" fn(*mut cef_browser_t, *mut cef_browser_t) -> c_int>,
    pub is_popup: Option<unsafe extern "C" fn(*mut cef_browser_t) -> c_int>,
    pub has_document: Option<unsafe extern "C" fn(*mut cef_browser_t) -> c_int>,
    pub get_main_frame: Option<unsafe extern "C" fn(*mut cef_browser_t) -> *mut cef_frame_t>,
    pub get_focused_frame: Option<unsafe extern "C" fn(*mut cef_browser_t) -> *mut cef_frame_t>,
    pub get_frame_by_identifier:
        Option<unsafe extern "C" fn(*mut cef_browser_t, *const cef_string_t) -> *mut cef_frame_t>,
    pub get_frame_by_name:
        Option<unsafe extern "C" fn(*mut cef_browser_t, *const cef_string_t) -> *mut cef_frame_t>,
    pub get_frame_count: Option<unsafe extern "C" fn(*mut cef_browser_t) -> usize>,
    pub get_frame_identifiers: Option<unsafe extern "C" fn(*mut cef_browser_t, cef_string_list_t)>,
    pub get_frame_names: Option<unsafe extern "C" fn(*mut cef_browser_t, cef_string_list_t)>,
}

#[repr(C)]
pub struct cef_browser_host_t {
    pub base: cef_base_ref_counted_t,
    pub get_browser: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> *mut cef_browser_t>,
    pub close_browser: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub try_close_browser: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub is_ready_to_be_closed: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub set_focus: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub get_window_handle:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> cef_window_handle_t>,
    pub get_opener_window_handle:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> cef_window_handle_t>,
    pub get_opener_identifier: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub has_view: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub get_client: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> *mut cef_client_t>,
    pub get_request_context:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> *mut cef_request_context_t>,
    pub can_zoom: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int) -> c_int>,
    pub zoom: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub get_default_zoom_level: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> f64>,
    pub get_zoom_level: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> f64>,
    pub set_zoom_level: Option<unsafe extern "C" fn(*mut cef_browser_host_t, f64)>,
    pub run_file_dialog: Option<
        unsafe extern "C" fn(
            *mut cef_browser_host_t,
            c_int,
            *const cef_string_t,
            *const cef_string_t,
            cef_string_list_t,
            *mut c_void,
        ),
    >,
    pub start_download: Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const cef_string_t)>,
    pub download_image: Option<
        unsafe extern "C" fn(
            *mut cef_browser_host_t,
            *const cef_string_t,
            c_int,
            u32,
            c_int,
            *mut c_void,
        ),
    >,
    pub print: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub print_to_pdf: Option<
        unsafe extern "C" fn(
            *mut cef_browser_host_t,
            *const cef_string_t,
            *const c_void,
            *mut c_void,
        ),
    >,
    pub find: Option<
        unsafe extern "C" fn(*mut cef_browser_host_t, *const cef_string_t, c_int, c_int, c_int),
    >,
    pub stop_finding: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub show_dev_tools: Option<
        unsafe extern "C" fn(
            *mut cef_browser_host_t,
            *const cef_window_info_t,
            *mut cef_client_t,
            *const cef_browser_settings_t,
            *const cef_point_t,
        ),
    >,
    pub close_dev_tools: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub has_dev_tools: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub send_dev_tools_message:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void, usize) -> c_int>,
    pub execute_dev_tools_method: Option<
        unsafe extern "C" fn(
            *mut cef_browser_host_t,
            c_int,
            *const cef_string_t,
            *mut cef_dictionary_value_t,
        ) -> c_int,
    >,
    pub add_dev_tools_message_observer:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *mut c_void) -> *mut c_void>,
    pub get_navigation_entries:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *mut c_void, c_int)>,
    pub replace_misspelling:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const cef_string_t)>,
    pub add_word_to_dictionary:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const cef_string_t)>,
    pub is_window_rendering_disabled:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub was_resized: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub was_hidden: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub notify_screen_info_changed: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub invalidate: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub send_external_begin_frame: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub send_key_event: Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void)>,
    pub send_mouse_click_event:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void, c_int, c_int, c_int)>,
    pub send_mouse_move_event:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void, c_int)>,
    pub send_mouse_wheel_event:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void, c_int, c_int)>,
    pub send_touch_event: Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void)>,
    pub send_capture_lost_event: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub notify_move_or_resize_started: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub get_windowless_frame_rate: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub set_windowless_frame_rate: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub ime_set_composition: Option<
        unsafe extern "C" fn(
            *mut cef_browser_host_t,
            *const cef_string_t,
            usize,
            *const c_void,
            *const c_void,
            *const c_void,
        ),
    >,
    pub ime_commit_text: Option<
        unsafe extern "C" fn(*mut cef_browser_host_t, *const cef_string_t, *const c_void, c_int),
    >,
    pub ime_finish_composing_text: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub ime_cancel_composition: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub drag_target_drag_enter:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *mut c_void, *const c_void, c_int)>,
    pub drag_target_drag_over:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void, c_int)>,
    pub drag_target_drag_leave: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub drag_target_drop: Option<unsafe extern "C" fn(*mut cef_browser_host_t, *const c_void)>,
    pub drag_source_ended_at:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int, c_int, c_int)>,
    pub drag_source_system_drag_ended: Option<unsafe extern "C" fn(*mut cef_browser_host_t)>,
    pub get_visible_navigation_entry:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> *mut c_void>,
    pub set_accessibility_state: Option<unsafe extern "C" fn(*mut cef_browser_host_t, cef_state_t)>,
    pub set_auto_resize_enabled: Option<
        unsafe extern "C" fn(*mut cef_browser_host_t, c_int, *const cef_size_t, *const cef_size_t),
    >,
    pub set_audio_muted: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub is_audio_muted: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub is_fullscreen: Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub exit_fullscreen: Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int)>,
    pub can_execute_chrome_command:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int) -> c_int>,
    pub execute_chrome_command:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t, c_int, cef_window_open_disposition_t)>,
    pub is_render_process_unresponsive:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> c_int>,
    pub get_runtime_style:
        Option<unsafe extern "C" fn(*mut cef_browser_host_t) -> cef_runtime_style_t>,
}

#[repr(C)]
pub struct cef_frame_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "C" fn(*mut cef_frame_t) -> c_int>,
    pub undo: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub redo: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub cut: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub copy: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub paste: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub paste_and_match_style: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub del: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub select_all: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub view_source: Option<unsafe extern "C" fn(*mut cef_frame_t)>,
    pub get_source: Option<unsafe extern "C" fn(*mut cef_frame_t, *mut c_void)>,
    pub get_text: Option<unsafe extern "C" fn(*mut cef_frame_t, *mut c_void)>,
    pub load_request: Option<unsafe extern "C" fn(*mut cef_frame_t, *mut cef_request_t)>,
    pub load_url: Option<unsafe extern "C" fn(*mut cef_frame_t, *const cef_string_t)>,
    pub execute_java_script: Option<
        unsafe extern "C" fn(*mut cef_frame_t, *const cef_string_t, *const cef_string_t, c_int),
    >,
    pub is_main: Option<unsafe extern "C" fn(*mut cef_frame_t) -> c_int>,
    pub is_focused: Option<unsafe extern "C" fn(*mut cef_frame_t) -> c_int>,
    pub get_name: Option<unsafe extern "C" fn(*mut cef_frame_t) -> cef_string_userfree_t>,
    pub get_identifier: Option<unsafe extern "C" fn(*mut cef_frame_t) -> cef_string_userfree_t>,
    pub get_parent: Option<unsafe extern "C" fn(*mut cef_frame_t) -> *mut cef_frame_t>,
    pub get_url: Option<unsafe extern "C" fn(*mut cef_frame_t) -> cef_string_userfree_t>,
    pub get_browser: Option<unsafe extern "C" fn(*mut cef_frame_t) -> *mut cef_browser_t>,
    pub get_v8_context: Option<unsafe extern "C" fn(*mut cef_frame_t) -> *mut c_void>,
    pub visit_dom: Option<unsafe extern "C" fn(*mut cef_frame_t, *mut c_void)>,
    pub create_urlrequest: Option<
        unsafe extern "C" fn(*mut cef_frame_t, *mut cef_request_t, *mut c_void) -> *mut c_void,
    >,
    pub send_process_message: Option<
        unsafe extern "C" fn(*mut cef_frame_t, cef_process_id_t, *mut cef_process_message_t),
    >,
}

#[repr(C)]
pub struct cef_process_message_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "C" fn(*mut cef_process_message_t) -> c_int>,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_process_message_t) -> c_int>,
    pub copy:
        Option<unsafe extern "C" fn(*mut cef_process_message_t) -> *mut cef_process_message_t>,
    pub get_name: Option<unsafe extern "C" fn(*mut cef_process_message_t) -> cef_string_userfree_t>,
    pub get_argument_list:
        Option<unsafe extern "C" fn(*mut cef_process_message_t) -> *mut cef_list_value_t>,
    pub get_shared_memory_region:
        Option<unsafe extern "C" fn(*mut cef_process_message_t) -> *mut c_void>,
}

#[repr(C)]
pub struct cef_list_value_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "C" fn(*mut cef_list_value_t) -> c_int>,
    pub is_owned: Option<unsafe extern "C" fn(*mut cef_list_value_t) -> c_int>,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_list_value_t) -> c_int>,
    pub is_same:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, *mut cef_list_value_t) -> c_int>,
    pub is_equal:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, *mut cef_list_value_t) -> c_int>,
    pub copy: Option<unsafe extern "C" fn(*mut cef_list_value_t) -> *mut cef_list_value_t>,
    pub set_size: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> c_int>,
    pub get_size: Option<unsafe extern "C" fn(*mut cef_list_value_t) -> usize>,
    pub clear: Option<unsafe extern "C" fn(*mut cef_list_value_t) -> c_int>,
    pub remove: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> c_int>,
    pub get_type: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> cef_value_type_t>,
    pub get_value: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> *mut c_void>,
    pub get_bool: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> c_int>,
    pub get_int: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> c_int>,
    pub get_double: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> f64>,
    pub get_string:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> cef_string_userfree_t>,
    pub get_binary: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> *mut c_void>,
    pub get_dictionary:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> *mut cef_dictionary_value_t>,
    pub get_list:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> *mut cef_list_value_t>,
    pub set_value: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize, *mut c_void) -> c_int>,
    pub set_null: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize) -> c_int>,
    pub set_bool: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize, c_int) -> c_int>,
    pub set_int: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize, c_int) -> c_int>,
    pub set_double: Option<unsafe extern "C" fn(*mut cef_list_value_t, usize, f64) -> c_int>,
    pub set_string:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, usize, *const cef_string_t) -> c_int>,
    pub set_binary:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, usize, *mut c_void) -> c_int>,
    pub set_dictionary: Option<
        unsafe extern "C" fn(*mut cef_list_value_t, usize, *mut cef_dictionary_value_t) -> c_int,
    >,
    pub set_list:
        Option<unsafe extern "C" fn(*mut cef_list_value_t, usize, *mut cef_list_value_t) -> c_int>,
}

#[repr(C)]
pub struct cef_dictionary_value_t {
    pub base: cef_base_ref_counted_t,
    pub is_valid: Option<unsafe extern "C" fn(*mut cef_dictionary_value_t) -> c_int>,
    pub is_owned: Option<unsafe extern "C" fn(*mut cef_dictionary_value_t) -> c_int>,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_dictionary_value_t) -> c_int>,
    pub is_same: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *mut cef_dictionary_value_t) -> c_int,
    >,
    pub is_equal: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *mut cef_dictionary_value_t) -> c_int,
    >,
    pub copy: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, c_int) -> *mut cef_dictionary_value_t,
    >,
    pub get_size: Option<unsafe extern "C" fn(*mut cef_dictionary_value_t) -> usize>,
    pub clear: Option<unsafe extern "C" fn(*mut cef_dictionary_value_t) -> c_int>,
    pub has_key:
        Option<unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> c_int>,
    pub get_keys:
        Option<unsafe extern "C" fn(*mut cef_dictionary_value_t, cef_string_list_t) -> c_int>,
    pub remove:
        Option<unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> c_int>,
    pub get_type: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> cef_value_type_t,
    >,
    pub get_value: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> *mut c_void,
    >,
    pub get_bool:
        Option<unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> c_int>,
    pub get_int:
        Option<unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> c_int>,
    pub get_double:
        Option<unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> f64>,
    pub get_string: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
        ) -> cef_string_userfree_t,
    >,
    pub get_binary: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> *mut c_void,
    >,
    pub get_dictionary: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
        ) -> *mut cef_dictionary_value_t,
    >,
    pub get_list: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
        ) -> *mut cef_list_value_t,
    >,
    pub set_value: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub set_null:
        Option<unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t) -> c_int>,
    pub set_bool: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t, c_int) -> c_int,
    >,
    pub set_int: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t, c_int) -> c_int,
    >,
    pub set_double: Option<
        unsafe extern "C" fn(*mut cef_dictionary_value_t, *const cef_string_t, f64) -> c_int,
    >,
    pub set_string: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
            *const cef_string_t,
        ) -> c_int,
    >,
    pub set_binary: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
            *mut c_void,
        ) -> c_int,
    >,
    pub set_dictionary: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
            *mut cef_dictionary_value_t,
        ) -> c_int,
    >,
    pub set_list: Option<
        unsafe extern "C" fn(
            *mut cef_dictionary_value_t,
            *const cef_string_t,
            *mut cef_list_value_t,
        ) -> c_int,
    >,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum cef_postdataelement_type_t {
    PDE_TYPE_EMPTY = 0,
    PDE_TYPE_BYTES = 1,
    PDE_TYPE_FILE = 2,
    PDE_TYPE_NUM_VALUES = 3,
}

#[repr(C)]
pub struct cef_post_data_element_t {
    pub base: cef_base_ref_counted_t,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_post_data_element_t) -> c_int>,
    pub set_to_empty: Option<unsafe extern "C" fn(*mut cef_post_data_element_t)>,
    pub set_to_file:
        Option<unsafe extern "C" fn(*mut cef_post_data_element_t, *const cef_string_t)>,
    pub set_to_bytes:
        Option<unsafe extern "C" fn(*mut cef_post_data_element_t, usize, *const c_void)>,
    pub get_type:
        Option<unsafe extern "C" fn(*mut cef_post_data_element_t) -> cef_postdataelement_type_t>,
    pub get_file:
        Option<unsafe extern "C" fn(*mut cef_post_data_element_t) -> cef_string_userfree_t>,
    pub get_bytes_count: Option<unsafe extern "C" fn(*mut cef_post_data_element_t) -> usize>,
    pub get_bytes:
        Option<unsafe extern "C" fn(*mut cef_post_data_element_t, usize, *mut c_void) -> usize>,
}

#[repr(C)]
pub struct cef_post_data_t {
    pub base: cef_base_ref_counted_t,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_post_data_t) -> c_int>,
    pub has_excluded_elements: Option<unsafe extern "C" fn(*mut cef_post_data_t) -> c_int>,
    pub get_element_count: Option<unsafe extern "C" fn(*mut cef_post_data_t) -> usize>,
    pub get_elements: Option<
        unsafe extern "C" fn(*mut cef_post_data_t, *mut usize, *mut *mut cef_post_data_element_t),
    >,
    pub remove_element:
        Option<unsafe extern "C" fn(*mut cef_post_data_t, *mut cef_post_data_element_t) -> c_int>,
    pub add_element:
        Option<unsafe extern "C" fn(*mut cef_post_data_t, *mut cef_post_data_element_t) -> c_int>,
    pub remove_elements: Option<unsafe extern "C" fn(*mut cef_post_data_t)>,
}

#[repr(C)]
pub struct cef_request_t {
    pub base: cef_base_ref_counted_t,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_request_t) -> c_int>,
    pub get_url: Option<unsafe extern "C" fn(*mut cef_request_t) -> cef_string_userfree_t>,
    pub set_url: Option<unsafe extern "C" fn(*mut cef_request_t, *const cef_string_t)>,
    pub get_method: Option<unsafe extern "C" fn(*mut cef_request_t) -> cef_string_userfree_t>,
    pub set_method: Option<unsafe extern "C" fn(*mut cef_request_t, *const cef_string_t)>,
    pub set_referrer: Option<
        unsafe extern "C" fn(*mut cef_request_t, *const cef_string_t, cef_referrer_policy_t),
    >,
    pub get_referrer_url: Option<unsafe extern "C" fn(*mut cef_request_t) -> cef_string_userfree_t>,
    pub get_referrer_policy:
        Option<unsafe extern "C" fn(*mut cef_request_t) -> cef_referrer_policy_t>,
    pub get_post_data: Option<unsafe extern "C" fn(*mut cef_request_t) -> *mut cef_post_data_t>,
    pub set_post_data: Option<unsafe extern "C" fn(*mut cef_request_t, *mut cef_post_data_t)>,
    pub get_header_map: Option<unsafe extern "C" fn(*mut cef_request_t, cef_string_multimap_t)>,
    pub set_header_map: Option<unsafe extern "C" fn(*mut cef_request_t, cef_string_multimap_t)>,
    pub get_header_by_name: Option<
        unsafe extern "C" fn(*mut cef_request_t, *const cef_string_t) -> cef_string_userfree_t,
    >,
    pub set_header_by_name: Option<
        unsafe extern "C" fn(*mut cef_request_t, *const cef_string_t, *const cef_string_t, c_int),
    >,
    pub set: Option<
        unsafe extern "C" fn(
            *mut cef_request_t,
            *const cef_string_t,
            *const cef_string_t,
            *mut cef_post_data_t,
            cef_string_multimap_t,
        ),
    >,
    pub get_flags: Option<unsafe extern "C" fn(*mut cef_request_t) -> c_int>,
    pub set_flags: Option<unsafe extern "C" fn(*mut cef_request_t, c_int)>,
    pub get_first_party_for_cookies:
        Option<unsafe extern "C" fn(*mut cef_request_t) -> cef_string_userfree_t>,
    pub set_first_party_for_cookies:
        Option<unsafe extern "C" fn(*mut cef_request_t, *const cef_string_t)>,
    pub get_resource_type: Option<unsafe extern "C" fn(*mut cef_request_t) -> cef_resource_type_t>,
    pub get_transition_type:
        Option<unsafe extern "C" fn(*mut cef_request_t) -> cef_transition_type_t>,
    pub get_identifier: Option<unsafe extern "C" fn(*mut cef_request_t) -> u64>,
}

#[repr(C)]
pub struct cef_response_t {
    pub base: cef_base_ref_counted_t,
    pub is_read_only: Option<unsafe extern "C" fn(*mut cef_response_t) -> c_int>,
    pub get_error: Option<unsafe extern "C" fn(*mut cef_response_t) -> cef_errorcode_t>,
    pub set_error: Option<unsafe extern "C" fn(*mut cef_response_t, cef_errorcode_t)>,
    pub get_status: Option<unsafe extern "C" fn(*mut cef_response_t) -> c_int>,
    pub set_status: Option<unsafe extern "C" fn(*mut cef_response_t, c_int)>,
    pub get_status_text: Option<unsafe extern "C" fn(*mut cef_response_t) -> cef_string_userfree_t>,
    pub set_status_text: Option<unsafe extern "C" fn(*mut cef_response_t, *const cef_string_t)>,
    pub get_mime_type: Option<unsafe extern "C" fn(*mut cef_response_t) -> cef_string_userfree_t>,
    pub set_mime_type: Option<unsafe extern "C" fn(*mut cef_response_t, *const cef_string_t)>,
    pub get_charset: Option<unsafe extern "C" fn(*mut cef_response_t) -> cef_string_userfree_t>,
    pub set_charset: Option<unsafe extern "C" fn(*mut cef_response_t, *const cef_string_t)>,
    pub get_header_by_name: Option<
        unsafe extern "C" fn(*mut cef_response_t, *const cef_string_t) -> cef_string_userfree_t,
    >,
    pub set_header_by_name: Option<
        unsafe extern "C" fn(*mut cef_response_t, *const cef_string_t, *const cef_string_t, c_int),
    >,
    pub get_header_map: Option<unsafe extern "C" fn(*mut cef_response_t, cef_string_multimap_t)>,
    pub set_header_map: Option<unsafe extern "C" fn(*mut cef_response_t, cef_string_multimap_t)>,
    pub get_url: Option<unsafe extern "C" fn(*mut cef_response_t) -> cef_string_userfree_t>,
    pub set_url: Option<unsafe extern "C" fn(*mut cef_response_t, *const cef_string_t)>,
}

// ---------------------------------------------------------------------------
// Dynamic / OS-loaded CEF entrypoints + runtime asset lookup
// ---------------------------------------------------------------------------

pub struct CefRuntimePaths {
    pub framework_dir: PathBuf,
    pub framework_binary: PathBuf,
    pub helper_exe: PathBuf,
}

#[cfg(target_os = "macos")]
pub fn locate_macos_paths() -> Result<CefRuntimePaths, String> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        for anc in exe.ancestors().skip(1).take(4) {
            candidates.push(anc.join("Frameworks"));
            candidates.push(anc.to_path_buf());
        }
    }
    candidates.push(PathBuf::from(env!("UI_CEF_PROFILE_DIR")));

    for dir in candidates {
        let fw = dir.join("Chromium Embedded Framework.framework");
        let bin = fw.join("Chromium Embedded Framework");
        let helper = dir.join("ui Helper.app/Contents/MacOS/ui Helper");
        if bin.exists() && helper.exists() {
            return Ok(CefRuntimePaths {
                framework_dir: fw,
                framework_binary: bin,
                helper_exe: helper,
            });
        }
    }
    Err("could not locate Chromium Embedded Framework.framework and `ui Helper.app`".into())
}

#[allow(clippy::type_complexity)]
pub struct CefApi {
    _lib: *mut c_void,
    pub cef_api_hash: unsafe extern "C" fn(c_int, c_int) -> *const c_char,
    pub cef_execute_process:
        unsafe extern "C" fn(*const cef_main_args_t, *mut cef_app_t, *mut c_void) -> c_int,
    pub cef_initialize: unsafe extern "C" fn(
        *const cef_main_args_t,
        *const cef_settings_t,
        *mut cef_app_t,
        *mut c_void,
    ) -> c_int,
    pub cef_shutdown: unsafe extern "C" fn(),
    pub cef_do_message_loop_work: unsafe extern "C" fn(),
    pub cef_browser_host_create_browser: unsafe extern "C" fn(
        *const cef_window_info_t,
        *mut cef_client_t,
        *const cef_string_t,
        *const cef_browser_settings_t,
        *mut cef_dictionary_value_t,
        *mut cef_request_context_t,
    ) -> c_int,
    pub cef_register_scheme_handler_factory: unsafe extern "C" fn(
        *const cef_string_t,
        *const cef_string_t,
        *mut cef_scheme_handler_factory_t,
    ) -> c_int,
    pub cef_request_context_create_context: unsafe extern "C" fn(
        *const cef_request_context_settings_t,
        *mut c_void,
    ) -> *mut cef_request_context_t,
    pub cef_dictionary_value_create: unsafe extern "C" fn() -> *mut cef_dictionary_value_t,
    pub cef_list_value_create: unsafe extern "C" fn() -> *mut cef_list_value_t,
    pub cef_process_message_create:
        unsafe extern "C" fn(*const cef_string_t) -> *mut cef_process_message_t,
    pub cef_string_utf8_to_utf16:
        unsafe extern "C" fn(*const c_char, usize, *mut cef_string_utf16_t) -> c_int,
    pub cef_string_utf16_to_utf8:
        unsafe extern "C" fn(*const char16_t, usize, *mut cef_string_utf8_t) -> c_int,
    pub cef_string_utf16_clear: unsafe extern "C" fn(*mut cef_string_utf16_t),
    pub cef_string_utf8_clear: unsafe extern "C" fn(*mut cef_string_utf8_t),
    pub cef_string_userfree_utf16_free: unsafe extern "C" fn(cef_string_userfree_t),
    pub cef_string_list_alloc: unsafe extern "C" fn() -> cef_string_list_t,
    pub cef_string_list_size: unsafe extern "C" fn(cef_string_list_t) -> usize,
    pub cef_string_list_value:
        unsafe extern "C" fn(cef_string_list_t, usize, *mut cef_string_t) -> c_int,
    pub cef_string_list_free: unsafe extern "C" fn(cef_string_list_t),
    pub cef_string_multimap_alloc: unsafe extern "C" fn() -> cef_string_multimap_t,
    pub cef_string_multimap_size: unsafe extern "C" fn(cef_string_multimap_t) -> usize,
    pub cef_string_multimap_key:
        unsafe extern "C" fn(cef_string_multimap_t, usize, *mut cef_string_t) -> c_int,
    pub cef_string_multimap_value:
        unsafe extern "C" fn(cef_string_multimap_t, usize, *mut cef_string_t) -> c_int,
    pub cef_string_multimap_append: unsafe extern "C" fn(
        cef_string_multimap_t,
        *const cef_string_t,
        *const cef_string_t,
    ) -> c_int,
    pub cef_string_multimap_free: unsafe extern "C" fn(cef_string_multimap_t),
}

// SAFETY: Resolved CEF C function pointers and read-only dylib handle are thread-safe to share.
unsafe impl Send for CefApi {}
// SAFETY: Resolved CEF C function pointers and read-only dylib handle are thread-safe to share.
unsafe impl Sync for CefApi {}

#[cfg(unix)]
mod dl {
    use super::*;
    pub const RTLD_LAZY: c_int = 0x1;
    pub const RTLD_LOCAL: c_int = 0x4;
    #[cfg(target_os = "macos")]
    pub const RTLD_FIRST: c_int = 0x100;
    #[cfg(not(target_os = "macos"))]
    pub const RTLD_FIRST: c_int = 0;

    unsafe extern "C" {
        pub fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        pub fn dlerror() -> *const c_char;
    }
}

static CEF_API: OnceLock<CefApi> = OnceLock::new();

#[cfg(target_os = "macos")]
fn load_lib_handle() -> Result<*mut c_void, String> {
    let paths = locate_macos_paths()?;
    let c_path = std::ffi::CString::new(paths.framework_binary.to_string_lossy().as_bytes())
        .map_err(|_| "invalid framework path".to_owned())?;
    // SAFETY: Valid NUL-terminated path passed to POSIX `dlopen`.
    let handle = unsafe {
        dl::dlopen(
            c_path.as_ptr(),
            dl::RTLD_LAZY | dl::RTLD_LOCAL | dl::RTLD_FIRST,
        )
    };
    if handle.is_null() {
        // SAFETY: `dlerror` returns a static or thread-local C error string after failed `dlopen`.
        let err = unsafe { dl::dlerror() };
        let msg = if err.is_null() {
            "unknown dlopen error".to_owned()
        } else {
            // SAFETY: `err` is NUL-terminated when non-null.
            unsafe { CStr::from_ptr(err) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(format!(
            "dlopen({}) failed: {msg}",
            paths.framework_binary.display()
        ));
    }
    Ok(handle)
}

#[cfg(unix)]
unsafe fn resolve_sym<T>(lib: *mut c_void, name: &CStr) -> Result<T, String> {
    // SAFETY: Caller passes a valid `dlopen` handle and static NUL-terminated symbol name.
    let ptr = unsafe { dl::dlsym(lib, name.as_ptr()) };
    if ptr.is_null() {
        return Err(format!("missing CEF symbol `{}`", name.to_string_lossy()));
    }
    // SAFETY: `T` is a C function pointer whose signature matches the CEF C-API export.
    Ok(unsafe { std::mem::transmute_copy(&ptr) })
}

#[cfg(not(unix))]
fn load_lib_handle() -> Result<*mut c_void, String> {
    Err("dynamic loader not implemented for this OS".into())
}

impl CefApi {
    /// Loads the CEF dynamic library once, configures `CEF_API_VERSION`, and verifies `cef_api_hash`.
    pub fn get() -> Result<&'static Self, String> {
        if let Some(api) = CEF_API.get() {
            return Ok(api);
        }
        #[cfg(unix)]
        {
            let lib = load_lib_handle()?;
            // SAFETY: `lib` is a loaded CEF shared library handle; each symbol matches CEF 156 C-API.
            let api = unsafe {
                Self {
                    _lib: lib,
                    cef_api_hash: resolve_sym(lib, c"cef_api_hash")?,
                    cef_execute_process: resolve_sym(lib, c"cef_execute_process")?,
                    cef_initialize: resolve_sym(lib, c"cef_initialize")?,
                    cef_shutdown: resolve_sym(lib, c"cef_shutdown")?,
                    cef_do_message_loop_work: resolve_sym(lib, c"cef_do_message_loop_work")?,
                    cef_browser_host_create_browser: resolve_sym(
                        lib,
                        c"cef_browser_host_create_browser",
                    )?,
                    cef_register_scheme_handler_factory: resolve_sym(
                        lib,
                        c"cef_register_scheme_handler_factory",
                    )?,
                    cef_request_context_create_context: resolve_sym(
                        lib,
                        c"cef_request_context_create_context",
                    )?,
                    cef_dictionary_value_create: resolve_sym(lib, c"cef_dictionary_value_create")?,
                    cef_list_value_create: resolve_sym(lib, c"cef_list_value_create")?,
                    cef_process_message_create: resolve_sym(lib, c"cef_process_message_create")?,
                    cef_string_utf8_to_utf16: resolve_sym(lib, c"cef_string_utf8_to_utf16")?,
                    cef_string_utf16_to_utf8: resolve_sym(lib, c"cef_string_utf16_to_utf8")?,
                    cef_string_utf16_clear: resolve_sym(lib, c"cef_string_utf16_clear")?,
                    cef_string_utf8_clear: resolve_sym(lib, c"cef_string_utf8_clear")?,
                    cef_string_userfree_utf16_free: resolve_sym(
                        lib,
                        c"cef_string_userfree_utf16_free",
                    )?,
                    cef_string_list_alloc: resolve_sym(lib, c"cef_string_list_alloc")?,
                    cef_string_list_size: resolve_sym(lib, c"cef_string_list_size")?,
                    cef_string_list_value: resolve_sym(lib, c"cef_string_list_value")?,
                    cef_string_list_free: resolve_sym(lib, c"cef_string_list_free")?,
                    cef_string_multimap_alloc: resolve_sym(lib, c"cef_string_multimap_alloc")?,
                    cef_string_multimap_size: resolve_sym(lib, c"cef_string_multimap_size")?,
                    cef_string_multimap_key: resolve_sym(lib, c"cef_string_multimap_key")?,
                    cef_string_multimap_value: resolve_sym(lib, c"cef_string_multimap_value")?,
                    cef_string_multimap_append: resolve_sym(lib, c"cef_string_multimap_append")?,
                    cef_string_multimap_free: resolve_sym(lib, c"cef_string_multimap_free")?,
                }
            };
            api.verify_hash()?;
            let _ = CEF_API.set(api);
            Ok(CEF_API.get().expect("CefApi initialized"))
        }
        #[cfg(not(unix))]
        {
            Err("CefApi::get() not yet implemented on this OS".into())
        }
    }

    fn verify_hash(&self) -> Result<(), String> {
        // SAFETY: First `cef_api_hash(CEF_API_VERSION, 0)` call configures the ABI version in libcef.
        let ptr = unsafe { (self.cef_api_hash)(CEF_API_VERSION, 0) };
        if ptr.is_null() {
            return Err("cef_api_hash returned null".into());
        }
        // SAFETY: `cef_api_hash` returns a static NUL-terminated C string owned by libcef.
        let got = unsafe { CStr::from_ptr(ptr) }.to_string_lossy();
        if got != CEF_API_HASH_PLATFORM {
            return Err(format!(
                "CEF API hash mismatch for version {CEF_API_VERSION}: expected {CEF_API_HASH_PLATFORM}, libcef reported {got}"
            ));
        }
        Ok(())
    }

    pub fn cef_string(&self, s: &str) -> cef_string_t {
        let mut out = cef_string_t::default();
        if !s.is_empty() {
            // SAFETY: `s.as_ptr()` points to `s.len()` valid UTF-8 bytes and `out` is valid stack storage.
            unsafe {
                (self.cef_string_utf8_to_utf16)(s.as_ptr().cast::<c_char>(), s.len(), &mut out);
            }
        }
        out
    }

    /// Frees a `cef_string_t` allocated via `cef_string`.
    ///
    /// # Safety
    /// `s` must have been initialized by CEF string helpers (or zeroed).
    pub unsafe fn clear_cef_string(&self, s: &mut cef_string_t) {
        // SAFETY: Delegated to caller's guarantee that `s` is CEF-initialized or zeroed.
        unsafe { (self.cef_string_utf16_clear)(s) };
    }

    /// Converts a `cef_string_t` slice to an owned Rust `String`.
    ///
    /// # Safety
    /// If `s` is non-null, its `str` and `length` must refer to readable UTF-16 units.
    pub unsafe fn from_cef_string(&self, s: *const cef_string_t) -> String {
        if s.is_null() {
            return String::new();
        }
        // SAFETY: Caller guarantees `s` is a valid CEF UTF-16 string descriptor.
        let r = unsafe { &*s };
        if r.str.is_null() || r.length == 0 {
            return String::new();
        }
        // SAFETY: Caller guarantees `r.str` holds `r.length` initialized `u16` values.
        let units = unsafe { std::slice::from_raw_parts(r.str, r.length) };
        String::from_utf16_lossy(units)
    }

    /// Reads every entry of a `cef_string_list_t`.
    ///
    /// # Safety
    /// `list` must be a live string list allocated by CEF.
    pub unsafe fn string_list(&self, list: cef_string_list_t) -> Vec<String> {
        // SAFETY: caller guarantees `list` is a live CEF string list.
        let n = unsafe { (self.cef_string_list_size)(list) };
        (0..n)
            .map(|i| {
                let mut s = cef_string_t::default();
                // SAFETY: `i` is in range; CEF copies the entry into `s`.
                unsafe {
                    (self.cef_string_list_value)(list, i, &mut s);
                    let text = self.from_cef_string(&s);
                    if let Some(dtor) = s.dtor {
                        dtor(s.str);
                    }
                    text
                }
            })
            .collect()
    }

    /// Reads and frees a `cef_string_userfree_t` returned from CEF.
    ///
    /// # Safety
    /// `s` must either be null or a freshly returned `cef_string_userfree_t` from CEF.
    pub unsafe fn take_userfree_string(&self, s: cef_string_userfree_t) -> String {
        if s.is_null() {
            return String::new();
        }
        // SAFETY: `s` is a valid userfree CEF string descriptor until freed below.
        let text = unsafe { self.from_cef_string(s) };
        // SAFETY: `s` was allocated by CEF and has not been freed yet.
        unsafe { (self.cef_string_userfree_utf16_free)(s) };
        text
    }
}

/// Decrements a CEF refcounted object's reference count when non-null.
///
/// # Safety
/// `ptr` must be null or point to a valid CEF struct beginning with `cef_base_ref_counted_t`.
pub unsafe fn cef_release<T>(ptr: *mut T) {
    if ptr.is_null() {
        return;
    }
    let base = ptr.cast::<cef_base_ref_counted_t>();
    // SAFETY: Caller guarantees `ptr` begins with a valid `cef_base_ref_counted_t`.
    if let Some(release) = unsafe { (*base).release } {
        // SAFETY: Invoking CEF C-API `release(self)` transfers one ref away.
        unsafe {
            release(base);
        }
    }
}

/// Increments a CEF refcounted object's reference count when non-null.
///
/// # Safety
/// `ptr` must be null or point to a valid CEF struct beginning with `cef_base_ref_counted_t`.
pub unsafe fn cef_add_ref<T>(ptr: *mut T) {
    if ptr.is_null() {
        return;
    }
    let base = ptr.cast::<cef_base_ref_counted_t>();
    // SAFETY: Caller guarantees `ptr` begins with a valid `cef_base_ref_counted_t`.
    if let Some(add_ref) = unsafe { (*base).add_ref } {
        // SAFETY: Invoking CEF C-API `add_ref(self)`.
        unsafe {
            add_ref(base);
        }
    }
}

/// Checks if a given `Path` is inside a packaged macOS `.app/Contents/Frameworks` directory.
pub fn is_bundled_frameworks_dir(dir: &Path) -> bool {
    dir.ends_with("Contents/Frameworks")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_framework_and_verifies_api_hash() {
        let api = CefApi::get().expect("CefApi::get should load CEF and match hash");
        let mut s = api.cef_string("hello cef 156");
        // SAFETY: `s` was just initialized by `cef_string`.
        let roundtrip = unsafe { api.from_cef_string(&s) };
        assert_eq!(roundtrip, "hello cef 156");
        // SAFETY: `s` is cleared once before going out of scope.
        unsafe { api.clear_cef_string(&mut s) };
    }
}
