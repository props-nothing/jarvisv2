import json
import subprocess
import sys
import time
from pathlib import Path

try:
    import pyautogui
    pyautogui.FAILSAFE = True
    pyautogui.PAUSE    = 0.06
    _PYAUTOGUI = True
except ImportError:
    _PYAUTOGUI = False

try:
    import pyperclip
    _PYPERCLIP = True
except ImportError:
    _PYPERCLIP = False

def _base_dir() -> Path:
    if getattr(sys, "frozen", False):
        return Path(sys.executable).parent
    return Path(__file__).resolve().parent.parent

def _get_os() -> str:
    try:
        cfg = json.loads(
            (_base_dir() / "config" / "api_keys.json").read_text(encoding="utf-8")
        )
        return cfg.get("os_system", "windows").lower()
    except Exception:
        return "windows"


def _require_pyautogui():
    if not _PYAUTOGUI:
        raise RuntimeError("PyAutoGUI not installed. Run: pip install pyautogui")


# ── The destructive-clear guard ──────────────────────────────────────────────
# Ctrl+A + Delete is only ever safe inside a known text field. Nothing in this
# module can prove that (see `_clear_and_paste`), so the sequence is off by
# default and `_replace_field` — which types over the selection and never presses
# Delete — is used instead. Kept as a named flag so the reason is discoverable
# rather than the sequence being quietly deleted.
_CLEAR_ALLOWED = False


def _paste_text(text: str) -> None:
    _require_pyautogui()

    os_name = _get_os()
    paste_hotkey = ("command", "v") if os_name == "mac" else ("ctrl", "v")

    if _PYPERCLIP:
        pyperclip.copy(text)
        time.sleep(0.15)
        pyautogui.hotkey(*paste_hotkey)
        time.sleep(0.1)
    else:
        pyautogui.write(text, interval=0.03)


def _active_window_title() -> str:
    """Title of the window that currently has focus, or "" if unknown."""
    try:
        import pygetwindow as gw
        w = gw.getActiveWindow()
        return (w.title or "").strip() if w is not None else ""
    except Exception:
        return ""


def _focus_matches(app_name: str) -> bool:
    """Is the window we expect actually in front?

    Every step of the send sequence below is blind keystrokes into whatever has
    focus. Before this check there was no way to know that Outlook had come to
    the front; if anything else stole focus mid-sequence, Ctrl+A and Delete went
    to that instead.
    """
    title = _active_window_title().lower()
    if not title:
        return True          # cannot tell — do not block on a missing probe
    return app_name.lower() in title


def _clear_and_paste(text: str) -> None:
    """Replace the contents of a text field. DESTRUCTIVE — see the guard.

    Ctrl+A followed by Delete deletes whatever is selected. In a text field that
    is the field. In a message LIST — which is what Ctrl+A selects in Outlook,
    Explorer, or any folder view — it is every item in that list, and Delete
    then moves them all to the trash.

    That is not hypothetical: this function was called against Outlook to clear
    a search box, selected the entire mailbox, and deleted the user's mail.

    There is no reliable way to ask Windows "is the focused control a text
    box?", so the guard is a refusal rather than a cleverer probe: only clear
    when we know the sequence has reached a search box. `_search_in_app` passes
    allow_clear=False for exactly this reason.
    """
    if not _CLEAR_ALLOWED:
        raise RuntimeError(
            "_clear_and_paste() is disabled: Ctrl+A + Delete deletes every "
            "selected item, not just a field's contents."
        )
    _require_pyautogui()
    os_name = _get_os()
    select_all = ("command", "a") if os_name == "mac" else ("ctrl", "a")
    pyautogui.hotkey(*select_all)
    time.sleep(0.1)
    pyautogui.press("delete")
    time.sleep(0.1)
    _paste_text(text)


def _replace_field(text: str) -> None:
    """Overwrite a search box with `text`, WITHOUT the select-all-then-delete.

    Ctrl+A selects the field's contents; typing over the selection replaces it,
    because a text field has no other meaning for that. No Delete key is ever
    pressed, so nothing can be removed from a list.
    """
    _require_pyautogui()
    os_name = _get_os()
    select_all = ("command", "a") if os_name == "mac" else ("ctrl", "a")
    pyautogui.hotkey(*select_all)
    time.sleep(0.1)
    _paste_text(text)


def _open_app(app_name: str) -> bool:
    _require_pyautogui()
    os_name = _get_os()

    try:
        if os_name == "windows":
            pyautogui.press("win")
            time.sleep(0.5)
            _paste_text(app_name)
            time.sleep(0.6)
            pyautogui.press("enter")
            time.sleep(2.5)
            return _focus_matches(app_name)

        elif os_name == "mac":
            result = subprocess.run(
                ["open", "-a", app_name],
                capture_output=True, text=True, timeout=10,
            )
            if result.returncode != 0:
                result = subprocess.run(
                    ["open", "-a", f"{app_name}.app"],
                    capture_output=True, text=True, timeout=10,
                )
            time.sleep(2.5)
            return result.returncode == 0

        else: 
            launched = False
            for launcher in [
                ["gtk-launch", app_name.lower()],
                [app_name.lower()],
            ]:
                try:
                    subprocess.Popen(
                        launcher,
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                    )
                    launched = True
                    break
                except FileNotFoundError:
                    continue
            time.sleep(2.5)
            return launched

    except Exception as e:
        print(f"[SendMessage] ⚠️ Could not open {app_name}: {e}")
        return False


def _open_browser_url(url: str) -> bool:
    import webbrowser
    try:
        webbrowser.open(url)
        time.sleep(4.0) 
        return True
    except Exception as e:
        print(f"[SendMessage] ⚠️ Could not open browser: {e}")
        return False

def _search_in_app(query: str) -> None:
    """Open the app's search box and put `query` in it.

    Uses `_replace_field`, never `_clear_and_paste`: the previous version
    pressed Ctrl+A then Delete, which in Outlook selected and deleted the whole
    mailbox rather than the contents of a search field.
    """
    _require_pyautogui()
    os_name = _get_os()
    search_hotkey = ("command", "f") if os_name == "mac" else ("ctrl", "f")

    pyautogui.hotkey(*search_hotkey)
    time.sleep(0.5)
    _replace_field(query)
    time.sleep(1.0)

def _desktop_send(app_name: str, receiver: str, message: str) -> str:
    """Drive a desktop app's UI to send a message.

    REPORTS WHAT IT ACTUALLY KNOWS. The old version ended with an unconditional
    `return f"Message sent to {receiver} via {app_name}."` after blind
    keystrokes, with nothing verified at any step — no check that the app came
    to the front, that the recipient resolved, or that the field had focus. The
    caller then logged '✅' by searching for the word "sent" inside that same
    string, so the tick could not fail. The assistant told the user a mail had
    been sent and then argued with them when it had not.

    Every step below either verifies something or says so honestly. It still
    cannot prove delivery — reading the Sent folder back would need a vision
    round trip on every message — so it says "submitted", and names the steps it
    could not confirm, instead of claiming a fact it does not have.
    """
    if not _open_app(app_name):
        return f"Could not open {app_name} — nothing was sent."

    time.sleep(1.0)

    if not _focus_matches(app_name):
        current = _active_window_title() or "an unknown window"
        return (f"Refused: after trying to open {app_name}, the focused window "
                f"was '{current}'. Sending now would type into the wrong "
                f"window, so nothing was sent.")

    _search_in_app(receiver)
    pyautogui.press("enter")
    time.sleep(0.8)

    if not _focus_matches(app_name):
        return (f"Refused: focus left {app_name} before typing — nothing was sent.")

    _paste_text(message)
    time.sleep(0.2)
    pyautogui.press("enter")
    time.sleep(0.3)

    # Past the point of no return. State exactly what is and is not known.
    return (f"Submitted to {app_name} for {receiver} — the message was typed and "
            f"Enter pressed. Delivery is NOT confirmed; if it does not appear, "
            f"the recipient name may not have matched. Verify before relying on it.")

def _send_whatsapp(receiver: str, message: str) -> str:
    return _desktop_send("WhatsApp", receiver, message)

def _send_telegram(receiver: str, message: str) -> str:
    return _desktop_send("Telegram", receiver, message)

def _send_signal(receiver: str, message: str) -> str:
    return _desktop_send("Signal", receiver, message)


def _send_discord(receiver: str, message: str) -> str:
    return _desktop_send("Discord", receiver, message)


def _send_instagram(receiver: str, message: str) -> str:
    _require_pyautogui()

    if not _open_browser_url("https://www.instagram.com/direct/new/"):
        return "Could not open Instagram in browser — nothing was sent."

    _paste_text(receiver)
    time.sleep(1.5)

    pyautogui.press("down")
    time.sleep(0.3)
    pyautogui.press("enter")   
    time.sleep(0.4)

    for _ in range(4):
        pyautogui.press("tab")
        time.sleep(0.15)
    pyautogui.press("enter")
    time.sleep(2.0)

    _paste_text(message)
    time.sleep(0.2)
    pyautogui.press("enter")
    time.sleep(0.3)

    return (f"Submitted to Instagram for {receiver} — the compose flow was driven "
            f"but delivery is NOT confirmed. If nothing arrived, the recipient "
            f"may not have been selected.")


def _send_messenger(receiver: str, message: str) -> str:
    _require_pyautogui()

    if not _open_browser_url("https://www.messenger.com/"):
        return "Could not open Messenger in browser — nothing was sent."

    _search_in_app(receiver)
    time.sleep(0.5)
    pyautogui.press("down")
    time.sleep(0.3)
    pyautogui.press("enter")
    time.sleep(1.0)

    _paste_text(message)
    time.sleep(0.2)
    pyautogui.press("enter")
    time.sleep(0.3)

    return (f"Submitted to Messenger for {receiver} — delivery is NOT confirmed."
            )

_PLATFORM_MAP = [
    ({"whatsapp", "wp", "wapp"},              _send_whatsapp),
    ({"telegram", "tg"},                      _send_telegram),
    ({"instagram", "ig", "insta"},            _send_instagram),
    ({"signal"},                               _send_signal),
    ({"discord"},                              _send_discord),
    ({"messenger", "facebook", "fb"},         _send_messenger),
]


def _resolve_platform(platform_str: str):
    key = platform_str.lower().strip()
    for keywords, handler in _PLATFORM_MAP:
        if any(k in key for k in keywords):
            return handler
    return lambda r, m: _desktop_send(platform_str.strip().title(), r, m)


def send_message(
    parameters: dict,
    response=None,
    player=None,
    session_memory=None,
) -> str:
    params       = parameters or {}
    receiver     = params.get("receiver", "").strip()
    message_text = params.get("message_text", "").strip()
    platform     = params.get("platform", "whatsapp").strip()

    if not receiver:
        return "Please specify a recipient."
    if not message_text:
        return "Please specify the message content."
    if not _PYAUTOGUI:
        return "PyAutoGUI is not installed — cannot control the desktop."

    preview = message_text[:50] + ("…" if len(message_text) > 50 else "")
    print(f"[SendMessage] 📨 {platform} → {receiver}: {preview}")
    if player:
        player.write_log(f"[msg] {platform} → {receiver}")

    try:
        handler = _resolve_platform(platform)
        result  = handler(receiver, message_text)
    except Exception as e:
        result = f"Could not send message: {e}"

    # Classify by the OUTCOME, not by a substring of our own output. The old
    # line was `'✅' if 'sent' in result.lower() else '❌'`, which searched for
    # the word "sent" inside a sentence this module had just built — it printed
    # a tick for a message that never left the machine.
    low = result.lower()
    failed = low.startswith(("refused", "could not", "error")) or "nothing was sent" in low
    sent   = low.startswith("submitted") or "sent via" in low
    mark = "❌" if failed else ("⚠️" if sent else "ℹ️")
    print(f"[SendMessage] {mark} {result}")
    if player:
        player.write_log(f"[msg] {result}")

    return result


# ── Tool declaration (auto-discovered by core/action_loader.py) ──────────────
TOOL = {
    "name": "send_message",
    "description": (
        "Sends a text message by driving a desktop app's UI (WhatsApp, Telegram, "
        "Signal, Discord, Instagram, Messenger, or any named app such as Outlook). "
        "IMPORTANT: delivery is NOT confirmed — the tool types the message and "
        "presses Enter, but cannot read back the Sent folder. Report exactly what "
        "it returns and never tell the user a message was delivered when the "
        "result says it was only submitted or refused. It only works while the "
        "target app can take focus; if the user is doing something else it will "
        "refuse rather than type into the wrong window."
    ),
    "parameters": {
        "type": "OBJECT",
        "properties": {
            "receiver": {
                "type": "STRING",
                "description": "Recipient contact name, phone number, or email address"
            },
            "message_text": {
                "type": "STRING",
                "description": "The message to send"
            },
            "platform": {
                "type": "STRING",
                "description": "Platform: WhatsApp, Telegram, Signal, Discord, Instagram, Messenger, Outlook, etc."
            }
        },
        "required": [
            "receiver",
            "message_text",
            "platform"
        ]
    },
    "handler": send_message,
}
