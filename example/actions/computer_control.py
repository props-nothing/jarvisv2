#computer_control.py
import io
import json
import platform
import re
import string
import subprocess
import sys

if platform.system() == "Windows":
    _WIN_HIDE: dict = {"creationflags": subprocess.CREATE_NO_WINDOW}
else:
    _WIN_HIDE: dict = {}
import time
import random
from pathlib import Path

try:
    import pyautogui
    pyautogui.FAILSAFE = True
    pyautogui.PAUSE    = 0.05
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


_BASE         = _base_dir()
_CONFIG_PATH  = _BASE / "config" / "api_keys.json"
_MEMORY_PATH  = _BASE / "memory" / "long_term.json"

def _load_config() -> dict:
    try:
        return json.loads(_CONFIG_PATH.read_text(encoding="utf-8"))
    except Exception:
        return {}

def _platform_os() -> str:
    return {"Windows": "windows", "Darwin": "mac", "Linux": "linux"}.get(
        platform.system(), "linux"
    )

def _get_os() -> str:
    return _load_config().get("os_system", _platform_os()).lower()


def _get_api_key() -> str:
    return _load_config().get("gemini_api_key", "")

_SAFE_SCREENSHOT_ROOTS = (
    Path.home(),
)

def _safe_screenshot_path(requested: str | None) -> Path:
    fallback = Path.home() / "Desktop" / "jarvis_screenshot.png"
    if not requested:
        return fallback
    try:
        p = Path(requested).expanduser().resolve()
        for root in _SAFE_SCREENSHOT_ROOTS:
            if p.is_relative_to(root.resolve()):
                p.parent.mkdir(parents=True, exist_ok=True)
                return p
    except Exception:
        pass
    return fallback

def _require_pyautogui():
    if not _PYAUTOGUI:
        raise RuntimeError("PyAutoGUI not installed. Run: pip install pyautogui")

_FIRST_NAMES = [
    "Alex", "Jordan", "Taylor", "Morgan", "Casey", "Riley", "Drew", "Quinn",
    "Avery", "Blake", "Cameron", "Dakota", "Emerson", "Finley", "Harper",
]
_LAST_NAMES = [
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
    "Davis", "Wilson", "Moore", "Taylor", "Anderson", "Thomas", "Jackson",
]
_DOMAINS = ["gmail.com", "yahoo.com", "outlook.com", "proton.me", "mail.com"]


def _random_data(data_type: str) -> str:
    dt = data_type.lower().strip()

    if dt == "first_name":
        return random.choice(_FIRST_NAMES)

    if dt == "last_name":
        return random.choice(_LAST_NAMES)

    if dt == "name":
        return f"{random.choice(_FIRST_NAMES)} {random.choice(_LAST_NAMES)}"

    if dt == "email":
        first = random.choice(_FIRST_NAMES).lower()
        last  = random.choice(_LAST_NAMES).lower()
        num   = random.randint(10, 999)
        return f"{first}.{last}{num}@{random.choice(_DOMAINS)}"

    if dt == "username":
        return f"{random.choice(_FIRST_NAMES).lower()}{random.randint(100, 9999)}"

    if dt == "password":
        chars = string.ascii_letters + string.digits + "!@#$%"
        raw   = (
            random.choice(string.ascii_uppercase)
            + random.choice(string.digits)
            + random.choice("!@#$%")
            + "".join(random.choices(chars, k=9))
        )
        return "".join(random.sample(raw, len(raw)))

    if dt == "phone":
        return f"+1{random.randint(200,999)}{random.randint(1_000_000, 9_999_999)}"

    if dt == "birthday":
        y = random.randint(1980, 2000)
        m = random.randint(1, 12)
        d = random.randint(1, 28)
        return f"{m:02d}/{d:02d}/{y}"

    if dt == "address":
        num    = random.randint(100, 9999)
        street = random.choice(["Main St", "Oak Ave", "Park Blvd", "Elm St", "Cedar Ln"])
        return f"{num} {street}"

    if dt == "zip_code":
        return str(random.randint(10000, 99999))

    if dt == "city":
        return random.choice(["New York", "Los Angeles", "Chicago", "Houston", "Phoenix"])

    return f"random_{data_type}_{random.randint(1000, 9999)}"

def _user_profile() -> dict:
    """Read identity fields from long-term memory."""
    try:
        if _MEMORY_PATH.exists():
            data     = json.loads(_MEMORY_PATH.read_text(encoding="utf-8"))
            identity = data.get("identity", {})
            return {k: v.get("value", "") for k, v in identity.items()}
    except Exception:
        pass
    return {}

def _type(text: str, interval: float = 0.03) -> str:
    _require_pyautogui()
    time.sleep(0.3)
    pyautogui.typewrite(text, interval=interval)
    return f"Typed: {text[:60]}{'…' if len(text) > 60 else ''}"


def _smart_type(text: str, clear_first: bool = True) -> str:
    _require_pyautogui()
    if clear_first:
        _clear_field()
        time.sleep(0.1)

    if len(text) > 20 and _PYPERCLIP:
        pyperclip.copy(text)
        time.sleep(0.1)
        paste_key = "command" if _get_os() == "mac" else "ctrl"
        pyautogui.hotkey(paste_key, "v")
        return f"Smart-typed (clipboard): {text[:60]}{'…' if len(text) > 60 else ''}"

    pyautogui.typewrite(text, interval=0.04)
    return f"Smart-typed: {text[:60]}{'…' if len(text) > 60 else ''}"


def _click(x=None, y=None, button: str = "left", clicks: int = 1,
           verify: bool = True) -> str:
    """Click, and report honestly whether the pointer actually landed there.

    pyautogui.click() returns nothing and cannot fail loudly: a coordinate off
    the edge of the desktop, or on a monitor that no longer exists, is silently
    discarded. The old code returned f"Clicke d ({x}, {y})" regardless, which is
    how three wrong guesses at the same button were each reported as a
    successful click. Reading the pointer back afterwards is cheap and turns a
    silent miss into a sentence the model can act on.
    """
    _require_pyautogui()
    if x is not None and y is not None:
        target = (int(x), int(y))
        try:
            pyautogui.moveTo(target[0], target[1])
            time.sleep(0.05)
            after = pyautogui.position()
            if (int(after.x), int(after.y)) != target:
                return (f"Could not move the pointer to {target} — it stopped at "
                        f"({int(after.x)},{int(after.y)}). That coordinate is "
                        f"outside the desktop.")
        except Exception as e:
            return f"Click at {target} failed: {e}"
        pyautogui.click(target[0], target[1], button=button, clicks=clicks)
        verb = "Double-clicked" if clicks == 2 else "Clicked"
        return f"{verb} ({target[0]}, {target[1]}) [{button}]"

    pyautogui.click(button=button, clicks=clicks)
    pos = pyautogui.position()
    return f"Clicked at current position ({int(pos.x)}, {int(pos.y)}) [{button}]"


def _hotkey(*keys) -> str:
    _require_pyautogui()
    pyautogui.hotkey(*keys)
    return f"Hotkey: {'+'.join(keys)}"


def _press(key: str) -> str:
    _require_pyautogui()
    pyautogui.press(key)
    return f"Pressed: {key}"


def _scroll(direction: str = "down", amount: int = 3) -> str:
    _require_pyautogui()
    vertical   = direction in ("up", "down")
    clicks     = amount if direction in ("up", "right") else -amount
    pyautogui.scroll(clicks) if vertical else pyautogui.hscroll(clicks)
    return f"Scrolled {direction} ×{amount}"


def _move(x: int, y: int, duration: float = 0.3) -> str:
    _require_pyautogui()
    try:
        pyautogui.moveTo(x, y, duration=duration)
    except Exception as e:
        return f"Could not move to ({x}, {y}): {e}"
    p = pyautogui.position()
    if (int(p.x), int(p.y)) != (int(x), int(y)):
        return (f"Pointer stopped at ({int(p.x)},{int(p.y)}) instead of "
                f"({x},{y}) — that coordinate is outside the desktop.")
    return f"Mouse → ({x}, {y})"


def _drag(x1: int, y1: int, x2: int, y2: int, duration: float = 0.5) -> str:
    _require_pyautogui()
    for px, py in ((x1, y1), (x2, y2)):
        try:
            pyautogui.moveTo(px, py, duration=0.05)
        except Exception as e:
            return f"Drag target ({px},{py}) is not reachable: {e}"
        p = pyautogui.position()
        if (int(p.x), int(p.y)) != (int(px), int(py)):
            return (f"Drag target ({px},{py}) is outside the desktop — the "
                    f"pointer stopped at ({int(p.x)},{int(p.y)}).")
    pyautogui.moveTo(x1, y1, duration=0.2)
    pyautogui.dragTo(x2, y2, duration=duration, button="left")
    return f"Dragged ({x1},{y1}) → ({x2},{y2})"


def _clipboard_get() -> str:
    if _PYPERCLIP:
        return pyperclip.paste()
    _hotkey("ctrl", "c")
    time.sleep(0.2)
    return "(copied — pyperclip unavailable for read)"


def _clipboard_paste(text: str) -> str:
    if _PYPERCLIP:
        pyperclip.copy(text)
        time.sleep(0.1)
        _require_pyautogui()
        paste_key = "command" if _get_os() == "mac" else "ctrl"
        pyautogui.hotkey(paste_key, "v")
        return f"Pasted: {text[:60]}{'…' if len(text) > 60 else ''}"
    return "pyperclip not available"


def _screenshot(save_path: str | None = None, target: str = "active") -> str:
    """Save a screenshot. `target` is 'active', 'all', or a monitor number.

    pyautogui.screenshot() — what this used to call — captures ONLY the primary
    display. On a two-monitor machine that silently saved the wrong screen.
    """
    _require_pyautogui()
    path = _safe_screenshot_path(save_path)
    try:
        from core import screens
        img, geom = screens.capture_target(target)
        if img is None:
            img = pyautogui.screenshot()
            img.save(str(path))
            return f"Screenshot saved: {path} (primary screen only)"
        img.convert("RGB").save(str(path))
        label = geom.get("label", "")
        return f"Screenshot saved: {path} ({img.size[0]}x{img.size[1]}, {label})"
    except Exception as e:
        img = pyautogui.screenshot()
        img.save(str(path))
        return f"Screenshot saved: {path} (fallback, primary only — {e})"


def _clear_field() -> str:
    _require_pyautogui()
    select_key = "command" if _get_os() == "mac" else "ctrl"
    pyautogui.hotkey(select_key, "a")
    time.sleep(0.1)
    pyautogui.press("delete")
    return "Field cleared"

def _focus_window(title: str) -> str:
    os_name = _get_os()

    if os_name == "windows":
        try:
            script = f'(New-Object -ComObject WScript.Shell).AppActivate("{title}")'
            subprocess.run(
                ["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
                capture_output=True, timeout=5, **_WIN_HIDE,
            )
            time.sleep(0.3)
            # Say WHICH screen it landed on. On a multi-monitor machine that is
            # half of "where is the window I am talking about", and the model
            # has no other way to learn it.
            extra = ""
            try:
                from core import screens
                import pygetwindow as _gw
                w = _gw.getActiveWindow()
                if w is not None and screens.screen_count() > 1:
                    mon = screens.monitor_for_point(
                        int(w.left + w.width / 2), int(w.top + w.height / 2))
                    if mon:
                        extra = (f" on screen {mon} of {screens.screen_count()} "
                                 f"(window at {int(w.left)},{int(w.top)} "
                                 f"{int(w.width)}x{int(w.height)})")
            except Exception:
                pass
            return f"Focused window: {title}{extra}"
        except Exception as e:
            return f"focus_window (Windows) failed: {e}"

    if os_name == "mac":
        script = (
            f'tell application "System Events" to '
            f'set frontmost of (first process whose name contains "{title}") to true'
        )
        try:
            subprocess.run(
                ["osascript", "-e", script],
                capture_output=True, timeout=5,
            )
            time.sleep(0.3)
            return f"Focused window: {title}"
        except Exception as e:
            return f"focus_window (macOS) failed: {e}"

    if os_name == "linux":
        try:
            result = subprocess.run(
                ["wmctrl", "-a", title],
                capture_output=True, timeout=5,
            )
            if result.returncode == 0:
                time.sleep(0.3)
                return f"Focused window: {title}"
        except FileNotFoundError:
            pass
        try:
            result = subprocess.run(
                ["xdotool", "search", "--name", title, "windowactivate"],
                capture_output=True, timeout=5,
            )
            time.sleep(0.3)
            return f"Focused window: {title}"
        except FileNotFoundError:
            return "focus_window (Linux) requires wmctrl or xdotool"
        except Exception as e:
            return f"focus_window (Linux) failed: {e}"

    return f"focus_window: unknown OS '{os_name}'"

def _screen_find(description: str, monitor: int = 0) -> tuple[int, int] | None:
    """Locate a UI element and return VIRTUAL-space coordinates, or None.

    THE BUG THIS REPLACES
        This used to take a pyautogui screenshot — which covers only the
        primary display — and ask the model for "the center coordinates" of an
        element it had never been shown. On a two-screen machine the model
        answered confidently about a screen it could not see, and every retry
        produced a different number: (53,889), (910,666), (495,578) for one
        Spotify button, none of them the button.

    Three things are different now:
      1. the model is SHOWN the image it is asked about;
      2. the image covers every monitor, so an element on the second screen is
         in the picture at all;
      3. its answer is converted out of capture space with
         screens.capture_to_virtual(), so the number is in the same coordinate
         system pyautogui clicks in.

    Coordinates outside the desktop are rejected rather than clicked.
    """
    api_key = _get_api_key()
    if not api_key:
        print("[ComputerControl] ⚠️ No API key for screen_find")
        return None

    try:
        from google import genai
        from google.genai import types as gtypes

        _require_pyautogui()

        from core import screens
        if not screens.available():
            print("[ComputerControl] ⚠️ mss unavailable — cannot capture")
            return None

        img, geom = screens.capture_all(annotate=True)
        if img is None or not geom:
            return None

        img_bytes, mime = screens.encode(img, "JPEG", quality=85)
        if not img_bytes:
            return None

        vx, vy, vw, vh = screens.virtual_bounds()
        prompt = (
            f"This image shows your ENTIRE desktop: {geom['total']} monitor(s), "
            f"{geom['image_w']}x{geom['image_h']} image pixels. The red border "
            f"marks the edge of the picture and the label names each screen.\n"
            f"In the ORIGINAL desktop coordinates the whole area spans "
            f"x from {vx} to {vx + vw} and y from {vy} to {vy + vh}.\n"
            f"Locate the UI element described as: '{description}'.\n"
            f"Reply with ONLY the center coordinates in ORIGINAL desktop "
            f"pixels as: x,y\n"
            f"If the element is not visible anywhere, reply: NOT_FOUND\n"
            f"Coordinates must lie inside the area given above, negative "
            f"values included. Do not guess — say NOT_FOUND instead."
        )

        from core import gemini
        response = gemini.call(
            [gtypes.Part.from_bytes(data=img_bytes, mime_type=mime), prompt],
            tier=gemini.FAST, timeout_ms=20_000,
        )
        if response is None:
            return None

        text = (response.text or "").strip()
        if "NOT_FOUND" in text.upper():
            return None

        match = re.search(r"(-?\d+)\s*,\s*(-?\d+)", text)
        if not match:
            return None

        # Belt and braces: the prompt asked for original desktop pixels, but a
        # model still occasionally answers in image pixels. If the raw pair is
        # inside the desktop, trust it; otherwise treat it as capture-space and
        # convert. This is the check that stops a plausible-but-wrong number
        # from becoming a click on something unrelated.
        raw_x, raw_y = int(match.group(1)), int(match.group(2))
        if vw > 0 and vh > 0 and (vx <= raw_x < vx + vw and vy <= raw_y < vy + vh):
            return raw_x, raw_y

        if (0 <= raw_x <= geom["image_w"]) and (0 <= raw_y <= geom["image_h"]):
            converted = screens.capture_to_virtual(raw_x, raw_y, geom)
            if converted:
                return converted

        print(f"[ComputerControl] ⚠️ '{description}' → ({raw_x},{raw_y}) is "
              f"outside the desktop; treating as not found")
        return None

    except Exception as e:
        print(f"[ComputerControl] ⚠️ screen_find failed: {e}")

    return None

def computer_control(
    parameters: dict,
    response=None,
    player=None,
    session_memory=None,
) -> str:
    """
    Dispatch table for all computer control actions.

    parameters keys (all optional unless noted):
      action        : (required) one of the actions listed below
      text          : text to type or paste
      x, y          : screen coordinates
      button        : 'left' | 'right' (default: left)
      keys          : hotkey string, e.g. 'ctrl+c'
      key           : single key name, e.g. 'enter'
      direction     : 'up' | 'down' | 'left' | 'right'
      amount        : scroll amount (default: 3)
      seconds       : wait duration
      title         : window title fragment for focus_window
      description   : natural-language element description for screen_find/click
      type          : data type for random_data
      field         : memory field name for user_data
      clear_first   : bool, clear field before typing (default: true)
      path          : save path for screenshot (must be inside home dir)

    Actions:
      type          — type text at cursor
      smart_type    — clear field + type (clipboard-backed)
      click         — left click
      double_click  — double left click
      right_click   — right click
      move          — move mouse
      drag          — click-drag between two points
      hotkey        — key combination
      press         — single key
      scroll        — scroll the wheel
      copy          — read clipboard
      paste         — write + paste clipboard
      screenshot    — capture screen (safe path only)
      wait          — sleep N seconds
      clear_field   — select-all + delete
      focus_window  — bring window to foreground
      screen_find   — AI element finder (returns x,y)
      screen_click  — AI element finder + click
      random_data   — generate fake form data
      user_data     — pull real data from memory
    """
    params = parameters or {}
    action = params.get("action", "").lower().strip()

    if not action:
        return "No action specified for computer_control."

    if player:
        player.write_log(f"[Computer] {action}")

    print(f"[ComputerControl] ▶ {action}  {params}")

    try:

        if action == "type":
            return _type(params.get("text", ""))

        if action == "smart_type":
            return _smart_type(
                params.get("text", ""),
                clear_first=params.get("clear_first", True),
            )

        if action in ("click", "left_click"):
            return _click(params.get("x"), params.get("y"), "left", 1)

        if action == "double_click":
            return _click(params.get("x"), params.get("y"), "left", 2)

        if action == "right_click":
            return _click(params.get("x"), params.get("y"), "right", 1)

        if action == "move":
            return _move(int(params.get("x", 0)), int(params.get("y", 0)))

        if action == "drag":
            return _drag(
                int(params.get("x1", 0)), int(params.get("y1", 0)),
                int(params.get("x2", 0)), int(params.get("y2", 0)),
            )

        if action == "hotkey":
            raw  = params.get("keys", "")
            keys = [k.strip() for k in raw.split("+")] if isinstance(raw, str) else raw
            return _hotkey(*keys)

        if action == "press":
            return _press(params.get("key", "enter"))

        if action == "scroll":
            return _scroll(
                direction=params.get("direction", "down"),
                amount=int(params.get("amount", 3)),
            )

        if action == "copy":
            return _clipboard_get()

        if action == "paste":
            return _clipboard_paste(params.get("text", ""))

        if action == "screenshot":
            return _screenshot(params.get("path"), params.get("screens", "active"))

        if action == "screen_find":
            coords = _screen_find(params.get("description", ""))
            return f"{coords[0]},{coords[1]}" if coords else "NOT_FOUND"

        if action == "screen_click":
            desc = params.get("description", "")
            # Honour coordinates the caller supplied. The old version ignored
            # them completely and re-derived a fresh guess from a vision call,
            # so the model could look at a screenshot, work out that the button
            # is at (380,619), pass that in — and have it silently thrown away
            # and replaced by an unrelated number.
            px, py = params.get("x"), params.get("y")
            if px is not None and py is not None:
                try:
                    coords = (int(px), int(py))
                except (TypeError, ValueError):
                    coords = None
            else:
                coords = None
            if coords is None:
                coords = _screen_find(desc)
            if coords:
                time.sleep(0.2)
                result = _click(x=coords[0], y=coords[1])
                mon = None
                try:
                    from core import screens
                    mon = screens.monitor_for_point(coords[0], coords[1])
                except Exception:
                    pass
                where = f" on screen {mon}" if mon else ""
                return f"{result}{where} — '{desc}'"
            return f"Element not found on screen: '{desc}'"

        if action == "wait":
            secs = float(params.get("seconds", 1.0))
            secs = min(secs, 30.0)
            time.sleep(secs)
            return f"Waited {secs}s"

        if action == "clear_field":
            return _clear_field()

        if action == "focus_window":
            return _focus_window(params.get("title", ""))

        if action == "random_data":
            dt     = params.get("type", "name")
            result = _random_data(dt)
            print(f"[ComputerControl] 🎲 random {dt} → {result}")
            return result

        if action == "user_data":
            field   = params.get("field", "name")
            profile = _user_profile()
            value   = profile.get(field, "")
            if not value:
                value = _random_data(field)
                print(f"[ComputerControl] ⚠️ No '{field}' in memory, using random: {value}")
            return value

        return f"Unknown action: '{action}'"

    except Exception as e:
        print(f"[ComputerControl] ❌ {action}: {e}")
        return f"computer_control '{action}' failed: {e}"


# ── Tool declaration (auto-discovered by core/action_loader.py) ──────────────
TOOL = {
    "name": "computer_control",
    "description": "Direct computer control: type, click, hotkeys, scroll, move mouse, screenshots, find elements on screen. Coordinates are in desktop pixels and may be NEGATIVE — on a multi-monitor machine the screen to the left of the primary one occupies negative x, and one above it negative y. screen_find reports coordinates in this same system, so pass its answer straight into screen_click.",
    "parameters": {
        "type": "OBJECT",
        "properties": {
            "action": {
                "type": "STRING",
                "description": "type | smart_type | click | double_click | right_click | hotkey | press | scroll | move | copy | paste | screenshot | wait | clear_field | focus_window | screen_find | screen_click | random_data | user_data"
            },
            "text": {
                "type": "STRING",
                "description": "Text to type or paste"
            },
            "x": {
                "type": "INTEGER",
                "description": "X coordinate in desktop pixels. May be negative on a multi-monitor setup (a screen positioned left of the primary)."
            },
            "y": {
                "type": "INTEGER",
                "description": "Y coordinate in desktop pixels. May be negative on a multi-monitor setup (a screen positioned above the primary)."
            },
            "keys": {
                "type": "STRING",
                "description": "Key combination e.g. 'ctrl+c'"
            },
            "key": {
                "type": "STRING",
                "description": "Single key e.g. 'enter'"
            },
            "direction": {
                "type": "STRING",
                "description": "up | down | left | right"
            },
            "amount": {
                "type": "INTEGER",
                "description": "Scroll amount (default: 3)"
            },
            "seconds": {
                "type": "NUMBER",
                "description": "Seconds to wait"
            },
            "title": {
                "type": "STRING",
                "description": "Window title for focus_window"
            },
            "description": {
                "type": "STRING",
                "description": "Element description for screen_find/screen_click"
            },
            "screens": {
                "type": "STRING",
                "description": "screenshot only: 'active' (default, the screen with the focused window), 'window' (just that window), 'all' (every monitor), or a monitor number"
            },
            "type": {
                "type": "STRING",
                "description": "Data type for random_data"
            },
            "field": {
                "type": "STRING",
                "description": "Field for user_data: name|email|city"
            },
            "clear_first": {
                "type": "BOOLEAN",
                "description": "Clear field before typing (default: true)"
            },
            "path": {
                "type": "STRING",
                "description": "Save path for screenshot"
            }
        },
        "required": [
            "action"
        ]
    },
    "handler": computer_control,
}
