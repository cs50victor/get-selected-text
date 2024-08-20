use std::num::NonZeroUsize;

use accessibility_ng::{AXAttribute, AXUIElement};
use accessibility_sys_ng::{kAXFocusedUIElementAttribute, kAXSelectedTextAttribute};
use active_win_pos_rs::get_active_window;
use core_foundation::string::CFString;
use debug_print::debug_println;
use lru::LruCache;
use parking_lot::Mutex;

use crate::SelectedText;

static GET_SELECTED_TEXT_METHOD: Mutex<Option<LruCache<String, u8>>> = Mutex::new(None);

pub fn get_window_meta() -> (String, String) {
    match get_active_window() {
        Ok(window) => (window.app_name, window.title),
        Err(_) => {
            // user might be in the desktop / home view
            ("Empty Window".into(), "Empty Window".into())
        }
    }
}

pub fn in_finder_or_empty_window() -> bool {
    let (app_name, _) = get_window_meta();
    app_name == "Finder" || app_name == "Empty Window"
}

pub fn get_selected_text() -> Result<SelectedText, Box<dyn std::error::Error>> {
    if GET_SELECTED_TEXT_METHOD.lock().is_none() {
        let cache = LruCache::new(NonZeroUsize::new(100).unwrap());
        *GET_SELECTED_TEXT_METHOD.lock() = Some(cache);
    }
    let mut cache = GET_SELECTED_TEXT_METHOD.lock();
    let cache = cache.as_mut().unwrap();
    
    let (app_name, window_title) = get_window_meta();

    let no_active_app = app_name == "Empty Window";
    if app_name == "Finder" || no_active_app {
        match get_selected_file_paths_by_clipboard_using_applescript(no_active_app) {
            Ok(text) => {
                println!("file paths: {:?}", text.split("\n"));
                return Ok(SelectedText {
                    is_file_paths: true,
                    app_name: app_name,
                    text: text.split("\n").map(|t| t.to_owned()).collect::<Vec<String>>(),
                });
            }
            Err(e) => {
                debug_println!("get_selected_file_paths_by_clipboard_using_applescript failed: {:?}", e);
            }
        }
    }

    let mut selected_text = SelectedText {
        is_file_paths: false,
        app_name: app_name.clone(),
        text: vec![],
    };

    if let Some(text) = cache.get(&app_name) {
        if *text == 0 {
            let ax_text = get_selected_text_by_ax()?;
            if !ax_text.is_empty() {
                cache.put(app_name.clone(), 0);
                selected_text.text = vec![ax_text];
                return Ok(selected_text);
            }
        }
        let txt = get_selected_text_by_clipboard_using_applescript()?;
        selected_text.text = vec![txt];
        return Ok(selected_text);
    }
    match get_selected_text_by_ax() {
        Ok(txt) => {
            if !txt.is_empty() {
                cache.put(app_name.clone(), 0);
            }
            selected_text.text = vec![txt];
            Ok(selected_text)
        }
        Err(_) => match get_selected_text_by_clipboard_using_applescript() {
            Ok(txt) => {
                if !txt.is_empty() {
                    cache.put(app_name, 1);
                }
                selected_text.text = vec![txt];
                Ok(selected_text)
            }
            Err(e) => Err(e),
        },
    }
}

fn get_selected_text_by_ax() -> Result<String, Box<dyn std::error::Error>> {
    // debug_println!("get_selected_text_by_ax");
    let system_element = AXUIElement::system_wide();
    let Some(selected_element) = system_element
        .attribute(&AXAttribute::new(&CFString::from_static_string(
            kAXFocusedUIElementAttribute,
        )))
        .map(|element| element.downcast_into::<AXUIElement>())
        .ok()
        .flatten()
    else {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No selected element",
        )));
    };
    let Some(selected_text) = selected_element
        .attribute(&AXAttribute::new(&CFString::from_static_string(
            kAXSelectedTextAttribute,
        )))
        .map(|text| text.downcast_into::<CFString>())
        .ok()
        .flatten()
    else {
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No selected text",
        )));
    };
    Ok(selected_text.to_string())
}

const REGULAR_TEXT_COPY_APPLE_SCRIPT: &str = r#"
use AppleScript version "2.4"
use scripting additions
use framework "Foundation"
use framework "AppKit"

set savedAlertVolume to alert volume of (get volume settings)

-- Back up clipboard contents:
set savedClipboard to the clipboard

set thePasteboard to current application's NSPasteboard's generalPasteboard()
set theCount to thePasteboard's changeCount()

tell application "System Events"
    set volume alert volume 0
end tell

-- Copy selected text to clipboard:
tell application "System Events" to keystroke "c" using {command down}
delay 0.1 -- Without this, the clipboard may have stale data.

tell application "System Events"
    set volume alert volume savedAlertVolume
end tell

if thePasteboard's changeCount() is theCount then
    return ""
end if

set theSelectedText to the clipboard

set the clipboard to savedClipboard

theSelectedText
"#;

const FILE_PATH_COPY_APPLE_SCRIPT: &str = r#"
tell application "Finder"
	set selectedItems to selection
	
	if selectedItems is {} then
		return "" -- Return an empty string if no items are selected
	end if
	
	set itemPaths to {}
	repeat with anItem in selectedItems
		set filePath to POSIX path of (anItem as alias)
		-- Escape any existing double quotes in the file path
		set escapedPath to my replace_chars(filePath, "\"", "\\\"")
		-- Add the escaped and quoted path to the list
		set end of itemPaths to "\"" & escapedPath & "\""
	end repeat
	
	set AppleScript's text item delimiters to linefeed
	set pathText to itemPaths as text
	
	return pathText -- Return the pathText content
end tell

on replace_chars(this_text, search_string, replacement_string)
	set AppleScript's text item delimiters to the search_string
	set the item_list to every text item of this_text
	set AppleScript's text item delimiters to the replacement_string
	set this_text to the item_list as string
	set AppleScript's text item delimiters to ""
	return this_text
end replace_chars
"#;

const EMPTY_WINDOW_PATH_COPY_APPLE_SCRIPT: &str = r#"
tell application "Finder"
	set desktopPath to (path to desktop folder as text)
	set selectedItems to (get selection)
	
	if selectedItems is {} then
		return "" -- Return an empty string if no items are selected
	end if
	
	set itemPaths to {}
	repeat with anItem in selectedItems
		set filePath to POSIX path of (anItem as alias)
		-- Escape any existing double quotes in the file path
		set escapedPath to my replace_chars(filePath, "\"", "\\\"")
		-- Add the escaped and quoted path to the list
		set end of itemPaths to "\"" & escapedPath & "\""
	end repeat
	
	set AppleScript's text item delimiters to linefeed
	set pathText to itemPaths as text
	
	return pathText -- Return the pathText content
end tell

on replace_chars(this_text, search_string, replacement_string)
	set AppleScript's text item delimiters to the search_string
	set the item_list to every text item of this_text
	set AppleScript's text item delimiters to the replacement_string
	set this_text to the item_list as string
	set AppleScript's text item delimiters to ""
	return this_text
end replace_chars
"#;

fn get_selected_text_by_clipboard_using_applescript() -> Result<String, Box<dyn std::error::Error>>
{
    // debug_println!("get_selected_text_by_clipboard_using_applescript");
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(REGULAR_TEXT_COPY_APPLE_SCRIPT)
        .output()?;

    if output.status.success() {
        let content = String::from_utf8(output.stdout)?;
        let content = content.trim();
        Ok(content.to_string())
    } else {
        let err = output
            .stderr
            .into_iter()
            .map(|c| c as char)
            .collect::<String>()
            .into();
        Err(err)
    }
}

fn get_selected_file_paths_by_clipboard_using_applescript(for_empty_window: bool
) -> Result<String, Box<dyn std::error::Error>> {
    // debug_println!("get_selected_text_by_clipboard_using_applescript");
    let mut binding = std::process::Command::new("osascript");
    let mut cmd = binding.arg("-e");

    if for_empty_window {
        cmd.arg(EMPTY_WINDOW_PATH_COPY_APPLE_SCRIPT);
    } else {
        cmd.arg(FILE_PATH_COPY_APPLE_SCRIPT);
    };

    let output = cmd.output()?;

    if output.status.success() {
        let content = String::from_utf8(output.stdout)?;
        let content = content.trim();
        Ok(content.to_string())
    } else {
        let err = output
            .stderr
            .into_iter()
            .map(|c| c as char)
            .collect::<String>()
            .into();
        Err(err)
    }
}
