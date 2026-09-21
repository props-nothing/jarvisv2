"""
Monitor geometry — one coordinate space shared by the eyes and the hands.

WHY THIS EXISTS
    On this machine the second display sits at left = -1920. That single fact
    produced a bug that looked like the model being bad at clicking.

    `_capture_screen()` grabbed `sct.monitors[1]` — whichever screen the OS
    happens to list first — and handed the model a 1920x1080 picture of THAT
    screen, resized. Separately, `_screen_find()` took a pyautogui screenshot,
    which covers only the PRIMARY display, and asked the model for "the center
    coordinates" of an element. The model had never been shown that image. A
    vision model asked "where is this button" on a screen it cannot see answers
    confidently and wrongly, and `screen_click` then clicked there — a fresh
    guess on every attempt, (53,889) then (910,666) then (495,578) for one
    button, none of them the button.

    So the fix is not "ask better". It is that sight and clicking must go
    through ONE function that knows where each monitor is sitting and converts
    between the picture the model was shown and the coordinates pyautogui
    clicks in.

THE TWO SPACES
    capture space — pixels of the image handed to the model, origin top-left of
                    THAT capture. Always starts at (0,0); its size is whatever
                    region was captured, after resizing.
    virtual space — pyautogui's coordinate system, spanning every monitor. The
                    primary display is (0,0)-(w,h); a monitor sitting left of or
                    above it has a NEGATIVE left/top.

    `capture_to_virtual()` is the only bridge between them, and it has to be
    applied to the model's raw answer before anything else touches it.

MONITOR INDICES
    Follow mss, which is the convention the rest of the codebase already used:
        0     — every monitor combined (the whole virtual desktop)
        1..n  — the individual screens, in the order the OS reports them
    Index 1 is NOT necessarily the primary screen on a multi-monitor machine.
    `is_primary` is reported explicitly instead of being assumed from the index.
"""
from __future__ import annotations

import platform
import re
from typing import Optional

try:
    import mss
    import mss.tools
    _MSS = True
except ImportError:
    _MSS = False

try:
    import PIL.Image
    import PIL.ImageDraw
    _PIL = True
except ImportError:
    _PIL = False


def _factory():
    """mss renamed `mss.mss` to `mss.MSS`; use the new name without a warning."""
    return getattr(mss, "MSS", None) or getattr(mss, "mss")


# Defaults for what we hand the model. Bounded because these images are tokens.
VISION_MAX_W, VISION_MAX_H = 1280, 720    # ordinary "what is on my screen"
FIND_MAX_W,   FIND_MAX_H   = 1920, 1080   # element finding — needs the detail


def available() -> bool:
    return _MSS


# ── enumeration ──────────────────────────────────────────────────────────────

def list_monitors() -> list[dict]:
    """Every monitor, mss-style. [] when mss is unavailable. Never raises."""
    if not _MSS:
        return []
    try:
        with _factory()() as sct:
            out = []
            for i, m in enumerate(sct.monitors):
                entry = {
                    "index": i,
                    "left":   int(m["left"]),
                    "top":    int(m["top"]),
                    "width":  int(m["width"]),
                    "height": int(m["height"]),
                }
                if i > 0:
                    # mss exposes is_primary on Windows. Fall back to "the
                    # screen whose origin is 0,0", which is what the OS means.
                    primary = m.get("is_primary")
                    if primary is None:
                        primary = entry["left"] == 0 and entry["top"] == 0
                    entry["is_primary"] = bool(primary)
                    entry["name"] = m.get("name") or f"Screen {i}"
                out.append(entry)
            return out
    except Exception:
        return []


def screen_count() -> int:
    """Number of physical screens (excludes the combined index 0)."""
    return max(0, len(list_monitors()) - 1)


def virtual_bounds() -> tuple[int, int, int, int]:
    """(left, top, width, height) of the whole virtual desktop."""
    mons = list_monitors()
    if len(mons) > 1:
        c = mons[0]
        return c["left"], c["top"], c["width"], c["height"]
    if _MSS:
        try:
            import pyautogui
            w, h = pyautogui.size()
            return 0, 0, int(w), int(h)
        except Exception:
            pass
    return 0, 0, 0, 0


def monitor_for_point(x: int, y: int) -> Optional[int]:
    """Which screen index contains this virtual-space point, or None."""
    for m in list_monitors():
        if m["index"] == 0:
            continue
        if (m["left"] <= x < m["left"] + m["width"]
                and m["top"] <= y < m["top"] + m["height"]):
            return m["index"]
    return None


def resolve_monitor(requested: int = 0) -> int:
    """Clamp a requested monitor index to one that exists.

    The model routinely passes 1 meaning "the first screen" or even 0 meaning
    "the screen". Both are honoured; anything out of range falls back to the
    primary rather than raising inside a vision call.
    """
    mons = list_monitors()
    n = len(mons) - 1
    if n <= 0:
        return 0
    try:
        idx = int(requested)
    except Exception:
        idx = 0
    if 1 <= idx <= n:
        return idx
    for m in mons:
        if m.get("is_primary"):
            return m["index"]
    return 1


def _self_window_markers() -> tuple[str, ...]:
    """Names this app titles its own window with.

    `ui.py` uses `setWindowTitle(f"{name} — {APP_VERSION}")`, so our window is
    always the assistant name, a dash, then the version.
    """
    names = []
    try:
        import json
        cfg_path = Path(__file__).resolve().parent.parent / "config" / "api_keys.json"
        cfg = json.loads(cfg_path.read_text(encoding="utf-8"))
        nm = (cfg.get("assistant_name") or "").strip()
        if nm:
            names.append(nm)
    except Exception:
        pass
    # The shipped default, so this still works before the config exists.
    names.append("JARVIS")
    return tuple(dict.fromkeys(n for n in names if n))


# Our own window is titled "<name> — <version>": the name at the START, then a
# dash separator (em dash, but en dash and hyphen are accepted so a locale or a
# future edit cannot silently break the exclusion).
#
# Matching the name ANYWHERE in the title was the first attempt and it was too
# eager: "TELEPHONY.md - jarvis - Visual Studio Code" is a VS Code window
# belonging to a folder that happens to be called "jarvis", and excluding it
# meant the assistant refused to look at the window the user was actually in.
# Anchoring the name to the start of the title is what distinguishes our window
# from someone else's title that merely mentions us.
_SELF_TITLE_RE = re.compile(
    r"^\s*(?:" + "|".join(re.escape(n) for n in _self_window_markers())
    + r")\s*[\u2014\u2013-]\s*",
    re.IGNORECASE,
)


def _is_self_window(title: str) -> bool:
    t = (title or "").strip()
    if not t:
        return False
    return bool(_SELF_TITLE_RE.match(t))


def active_monitor() -> int:
    """The screen the user is actually looking at — where the window with focus
    is sitting, falling back to where the mouse is.

    This is the answer to "what is on my screen" on a multi-monitor machine.
    Capturing a fixed monitor means describing the wrong display half the time,
    and the old code did exactly that: it took whichever screen mss listed
    first.

    Our own HUD is skipped: it is a foreground window, so leaving it in means
    "the focused window's screen" becomes "wherever I happen to be drawn".
    Returns 0 (whole desktop) when there is only one screen, so the
    single-monitor behaviour is unchanged.
    """
    if screen_count() <= 1:
        return 0
    try:
        import pygetwindow as gw
        w = gw.getActiveWindow()
        if w is not None and not _is_self_window(getattr(w, "title", "")):
            cx = int(w.left + w.width / 2)
            cy = int(w.top + w.height / 2)
            idx = monitor_for_point(cx, cy)
            if idx is not None:
                return idx
    except Exception:
        pass
    try:
        import pyautogui
        p = pyautogui.position()
        idx = monitor_for_point(int(p.x), int(p.y))
        if idx is not None:
            return idx
    except Exception:
        pass
    return resolve_monitor(0)


def describe() -> str:
    """A line for the prompt: how many screens, and where they are."""
    mons = [m for m in list_monitors() if m["index"] > 0]
    if not mons:
        return "Display geometry is unavailable in this build."
    if len(mons) == 1:
        m = mons[0]
        return (f"There is 1 screen: {m['width']}x{m['height']} at "
                f"({m['left']},{m['top']}).")
    parts = []
    for m in mons:
        tag = "primary" if m.get("is_primary") else "secondary"
        parts.append(f"#{m['index']} {m['width']}x{m['height']} at "
                     f"({m['left']},{m['top']}) [{tag}]")
    return (f"There are {len(mons)} screens: " + "; ".join(parts) +
            ". Screen #1 is not necessarily the primary one.")


# ── capture ──────────────────────────────────────────────────────────────────

def _label_for(monitor: int, mons: list[dict]) -> str:
    if monitor == 0 or len(mons) <= 1:
        total = max(1, len(mons) - 1)
        if monitor < 0:
            total = 1
        return "ALL SCREENS" if total > 1 and monitor == 0 else "SCREEN"
    n = len(mons) - 1
    m = mons[monitor]
    tag = "PRIMARY" if m.get("is_primary") else "SECONDARY"
    return f"SCREEN {monitor}/{n} \u2014 {tag}"


def _annotate(img, label: str):
    """Mark the image as a complete screen and say which one it is.

    Two jobs. A thin border tells the model the picture is the whole screen
    rather than a crop, which is what stops it guessing at off-image content.
    The caption names the monitor, which is the difference between "the button
    is at 900,600" and "the button is at 900,600 on screen 2".
    """
    if not _PIL or not label:
        return img
    try:
        d = PIL.ImageDraw.Draw(img)
        w, h = img.size
        d.rectangle([0, 0, w - 1, h - 1], outline=(255, 64, 64), width=2)
        text = f" {label} "
        try:
            box = d.textbbox((0, 0), text)
            tw, th = box[2] - box[0], box[3] - box[1]
        except Exception:
            tw, th = 8 * len(text), 12
        pad = 4
        d.rectangle([0, 0, tw + pad * 2, th + pad * 2], fill=(255, 64, 64))
        d.text((pad, pad), text, fill=(255, 255, 255))
    except Exception:
        pass
    return img


def capture(monitor: int = 0,
            annotate: bool = True,
            max_w: int = VISION_MAX_W,
            max_h: int = VISION_MAX_H):
    """Grab a screen and return (PIL image, geometry).

    geometry is what every later conversion needs:
        origin_left / origin_top  where this capture sits in virtual space
        capture_w / capture_h     region size before resizing
        image_w / image_h         size of the image the model is shown
        monitor / total / label   which screen this is, for the caption

    Returns (None, {}) if the capture failed, so callers degrade instead of
    raising inside a Live session.
    """
    if not _MSS:
        return None, {}
    mons = list_monitors()
    idx = monitor if 0 <= monitor < len(mons) else 0
    if idx == 0 and len(mons) <= 1:
        idx = 0
    try:
        with _factory()() as sct:
            target = sct.monitors[idx] if idx < len(sct.monitors) else sct.monitors[0]
            shot = sct.grab(target)
            left, top = int(shot.left), int(shot.top)
            cap_w, cap_h = int(shot.width), int(shot.height)
            if _PIL:
                img = PIL.Image.frombytes("RGB", shot.size, shot.rgb)
            else:
                img = shot
    except Exception as e:
        print(f"[Screens] \u26a0\ufe0f  Capture failed: {e}")
        return None, {}

    img_w, img_h = cap_w, cap_h
    if _PIL:
        if max_w and max_h and (img.width > max_w or img.height > max_h):
            img.thumbnail((max_w, max_h), PIL.Image.BILINEAR)
        if annotate:
            img = _annotate(img, _label_for(idx, mons))
        img_w, img_h = img.size

    geom = {
        "origin_left": left,
        "origin_top":  top,
        "capture_w":   cap_w,
        "capture_h":   cap_h,
        "image_w":     img_w,
        "image_h":     img_h,
        "monitor":     idx,
        "total":       max(len(mons) - 1, 1),
        "label":       _label_for(idx, mons),
    }
    return img, geom


def capture_all(annotate: bool = True,
                max_w: int = FIND_MAX_W,
                max_h: int = FIND_MAX_H):
    """Grab every monitor as one image — used by element finding.

    The whole desktop rather than one screen, because a window the user is
    talking about can be on either monitor and the model has to be able to see
    it to locate anything in it.
    """
    mons = list_monitors()
    if len(mons) <= 2:
        return capture(1 if len(mons) > 1 else 0, annotate=annotate,
                       max_w=max_w, max_h=max_h)
    return capture(0, annotate=annotate, max_w=max_w, max_h=max_h)


def _intersect(a, b):
    """Intersection of two (left, top, w, h) rects, or None."""
    al, at, aw, ah = a
    bl, bt, bw, bh = b
    l = max(al, bl)
    t = max(at, bt)
    r = min(al + aw, bl + bw)
    bo = min(at + ah, bt + bh)
    if r <= l or bo <= t:
        return None
    return l, t, r - l, bo - t


def _window_rect():
    """(left, top, w, h) of the window with focus, in virtual space, or None.

    Our own HUD is excluded for the same reason as in `active_monitor`: it is
    usually the focused window, and cropping the assistant's own face is never
    what "look at my screen" means. Cropped to the monitor it sits on unless it
    genuinely spans several, so a window straddling two screens still yields a
    usable picture.
    """
    if not _PIL:
        return None
    try:
        import pygetwindow as gw
        w = gw.getActiveWindow()
        if w is None:
            return None
        if _is_self_window(getattr(w, "title", "")):
            return None
        rect = (int(w.left), int(w.top), int(w.width), int(w.height))
    except Exception:
        return None
    if rect[2] < 200 or rect[3] < 150:      # a tooltip or a sliver — not useful
        return None
    mons = [m for m in list_monitors() if m["index"] > 0]
    if not mons:
        return rect
    host = monitor_for_point(rect[0] + rect[2] // 2, rect[1] + rect[3] // 2)
    if host is None:
        return rect
    m = next((x for x in mons if x["index"] == host), None)
    if m is None:
        return rect
    clipped = _intersect(rect, (m["left"], m["top"], m["width"], m["height"]))
    return clipped or rect


def grab_region(left: int, top: int, w: int, h: int,
                annotate: bool = False,
                label: str = "",
                max_w: int = FIND_MAX_W,
                max_h: int = FIND_MAX_H):
    """Capture an arbitrary virtual-space rectangle.

    WHY mss AND NOT pyautogui HERE
        pyautogui.screenshot(region=...) looks like the obvious choice — it takes
        negative coordinates, returns the right SIZE, and raises nothing when
        you give it a rectangle on a monitor sitting to the LEFT of the primary.
        It just returns a completely black image. Measured on this machine:

            pyautogui.screenshot(region=(-1920,0,1920,1080))  → mean pixel 0.0
            mss.grab({... same rect ...})                     → mean pixel 116.5

        So a window crop on the secondary display would have handed the model a
        blank picture, and it would have described a black screen in good faith.
        That is the same failure shape as the bug this module exists to fix —
        plausible, silent, and wrong.

        mss positions sub-regions correctly at negative origins; this is verified
        rather than assumed: a sub-region grabbed from the secondary monitor is
        byte-identical to the matching crop of the whole monitor, and likewise
        on the primary.

    The result is clipped to the virtual desktop first, so a rectangle partly
    off-screen yields the visible part instead of an error.
    """
    if not _MSS:
        return None, {}
    vl, vt, vw, vh = virtual_bounds()
    l, t, w, h = int(left), int(top), int(w), int(h)
    if vw > 0 and vh > 0:
        l = max(l, vl)
        t = max(t, vt)
        w = min(w, (vl + vw) - l)
        h = min(h, (vt + vh) - t)
    if w <= 0 or h <= 0:
        print(f"[Screens] \u26a0\ufe0f  Region {left},{top} {w}x{h} is entirely "
              f"off-screen — nothing to capture")
        return None, {}
    try:
        with _factory()() as sct:
            shot = sct.grab({"left": l, "top": t, "width": w, "height": h})
            cap_w, cap_h = int(shot.width), int(shot.height)
            if _PIL:
                img = PIL.Image.frombytes("RGB", shot.size, shot.rgb)
            else:
                img = shot
    except Exception as e:
        print(f"[Screens] \u26a0\ufe0f  Region capture failed: {e}")
        return None, {}

    if _PIL:
        if max_w and max_h and (img.width > max_w or img.height > max_h):
            img.thumbnail((max_w, max_h), PIL.Image.BILINEAR)
        if annotate and label:
            img = _annotate(img, label)
        img_w, img_h = img.size
    else:
        img_w, img_h = cap_w, cap_h

    geom = {
        "origin_left": l,
        "origin_top":  t,
        "capture_w":   cap_w,
        "capture_h":   cap_h,
        "image_w":     img_w,
        "image_h":     img_h,
        "monitor":     monitor_for_point(l + w // 2, t + h // 2) or 0,
        "total":       max(screen_count(), 1),
        "label":       label or "WINDOW",
    }
    return img, geom


def capture_target(target: str = "active"):
    """Capture what the user means by "my screen". Returns (image, geometry).

    active — the whole MONITOR holding the window with focus. This is the
             default because it is literally "my screen", and because it is
             exactly what the old code did on a single-monitor machine: the
             previous behaviour is preserved wherever it was already correct,
             and only the choice of WHICH screen changes when there are several.
    window — just that window, cropped. More detail for reading small text, at
             the cost of everything around it (taskbar, other windows).
    all    — every monitor as one picture.
    a number — that specific screen index.
    """
    t = str(target).strip().lower()
    if t in ("all", "every", "everyone", "desktop"):
        return capture_all(annotate=True)
    if t.isdigit():
        return capture(resolve_monitor(int(t)), annotate=True)
    if screen_count() <= 1:
        # Single monitor: identical to the original behaviour.
        return capture(0, annotate=True)

    rect = _window_rect()
    host = None
    if rect:
        host = monitor_for_point(rect[0] + rect[2] // 2, rect[1] + rect[3] // 2)
    if host is None:
        host = active_monitor()
    if host in (None, 0):
        host = resolve_monitor(0)

    if t in ("window", "active_window", "focused"):
        if rect:
            mons = list_monitors()
            m = next((x for x in mons if x["index"] == host), None)
            base = _label_for(host, mons) if m else "WINDOW"
            return grab_region(rect[0], rect[1], rect[2], rect[3],
                               annotate=True, label=f"{base} — ACTIVE WINDOW",
                               max_w=VISION_MAX_W, max_h=VISION_MAX_H)
        # No window rect available — fall through to the whole screen.
    return capture(host, annotate=True)


# ── the bridge ───────────────────────────────────────────────────────────────

def capture_to_virtual(mx: float, my: float, geom: dict) -> Optional[tuple[int, int]]:
    """Model's answer (capture space) → pyautogui coordinates (virtual space).

    Ratio-based rather than a stored scale factor, so it stays right whatever
    the resize did and whether or not the two axes shrank equally. Returns None
    if the result falls outside the virtual desktop, which is the check that
    stops a confident wrong answer from being turned into a click somewhere
    arbitrary.
    """
    if not geom:
        return None
    iw = int(geom.get("image_w") or 0)
    ih = int(geom.get("image_h") or 0)
    if iw <= 0 or ih <= 0:
        return None
    cap_w = float(geom.get("capture_w") or iw)
    cap_h = float(geom.get("capture_h") or ih)
    x = float(geom.get("origin_left", 0)) + (float(mx) / iw) * cap_w
    y = float(geom.get("origin_top", 0)) + (float(my) / ih) * cap_h
    vx, vy = int(round(x)), int(round(y))

    left, top, w, h = virtual_bounds()
    if w > 0 and h > 0:
        if not (left <= vx < left + w and top <= vy < top + h):
            print(f"[Screens] \u26a0\ufe0f  ({mx},{my}) maps to ({vx},{vy}), "
                  f"outside the desktop {left},{top} {w}x{h} — discarding")
            return None
    return vx, vy


def encode(img, fmt: str = "PNG", quality: int = 82) -> tuple[bytes, str]:
    """PIL image → (bytes, mime). Callers own the format they need."""
    if img is None:
        return b"", ""
    if not _PIL or not hasattr(img, "save"):
        return b"", ""
    import io
    buf = io.BytesIO()
    try:
        if fmt.upper() == "JPEG":
            img.convert("RGB").save(buf, format="JPEG", quality=quality,
                                    optimize=False)
            return buf.getvalue(), "image/jpeg"
        img.save(buf, format="PNG")
        return buf.getvalue(), "image/png"
    except Exception as e:
        print(f"[Screens] \u26a0\ufe0f  Encode failed: {e}")
        return b"", ""
