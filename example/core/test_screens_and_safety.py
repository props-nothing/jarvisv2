"""
Regression tests for the multi-monitor and false-success fixes.

WHY THESE EXIST
    No test in this repo would have caught either bug, because the bugs were not
    in the arithmetic — they were in what got handed to whom. `_screen_find`
    asked a model for coordinates on an image it had never been shown, and
    `send_message` checked for the word "sent" inside a string it had just
    written. Both pass any test that only exercises the happy path.

    So these tests assert the INVARIANTS that were violated:
      * a coordinate derived from a capture must land back inside the region
        that capture covers — including on a monitor with a negative origin;
      * a picture handed to the model must actually contain the region the
        answer will be interpreted in;
      * a success message must not be able to be produced without an outcome.

    Run:  .\\.venv\\Scripts\\python.exe core\\test_screens_and_safety.py
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

# The status lines carry arrow glyphs, and a legacy Windows console runs cp1252
# where those raise UnicodeEncodeError. Same guard main.py uses, for the same
# reason: without it this test dies on its own output in a Turkish or Russian
# locale rather than reporting anything.
for _s in ("stdout", "stderr"):
    _stream = getattr(sys, _s, None)
    try:
        if _stream is not None and hasattr(_stream, "reconfigure"):
            _stream.reconfigure(encoding="utf-8", errors="replace")
    except Exception:
        pass

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from core import screens  # noqa: E402

PASS, FAIL = [], []


def check(name: str, cond: bool, detail: str = "") -> None:
    (PASS if cond else FAIL).append(name)
    mark = "PASS" if cond else "FAIL"
    print(f"  [{mark}] {name}" + (f"  — {detail}" if detail and not cond else ""))


# ── the coordinate bridge ────────────────────────────────────────────────────
print("\n[coordinate bridge: capture space → virtual space]")

# The machine this was found on: primary at 0,0 and a second screen to its LEFT
# at -1920,0. The secondary is what mss listed FIRST (index 1), so a capture of
# "the first monitor" was a capture of the second display.
LEFT_MON = {
    "origin_left": -1920, "origin_top": 0,
    "capture_w": 1920, "capture_h": 1080,
    "image_w": 1920, "image_h": 1080,
    "monitor": 1, "total": 2, "label": "SCREEN 1/2 — SECONDARY",
}
PRIMARY = {
    "origin_left": 0, "origin_top": 0,
    "capture_w": 1920, "capture_h": 1080,
    "image_w": 1280, "image_h": 720,       # resized, as the real path does
    "monitor": 2, "total": 2, "label": "SCREEN 2/2 — PRIMARY",
}

# Top-left of the left-hand monitor must map to its own negative origin.
v = screens.capture_to_virtual(0, 0, LEFT_MON)
check("left monitor: image (0,0) → virtual (-1920,0)", v == (-1920, 0), f"got {v}")

# Bottom-right PIXEL of the left-hand monitor must map to its own far edge.
# Note the last addressable pixel is (width-1, height-1): a click at the
# exclusive corner (1920,1080) is one pixel past the screen and is correctly
# rejected — the earlier assertion here used the exclusive corner and was
# itself off by one.
v = screens.capture_to_virtual(1919, 1079, LEFT_MON)
check("left monitor: last pixel → virtual (-1,1079)", v == (-1, 1079), f"got {v}")

# And the exclusive corner must be REJECTED — it is outside any screen.
check("left monitor: exclusive corner rejected",
      screens.capture_to_virtual(1920, 1080, LEFT_MON) is None)

# A coordinate in the middle of the left monitor stays negative.
v = screens.capture_to_virtual(960, 540, LEFT_MON)
check("left monitor: centre → virtual (-960,540)", v == (-960, 540), f"got {v}")

# The resize must be honoured: a 1280x720 image of a 1920x1080 screen.
v = screens.capture_to_virtual(640, 360, PRIMARY)
check("resized primary: image centre → virtual (960,540)", v == (960, 540), f"got {v}")

v = screens.capture_to_virtual(1279, 719, PRIMARY)
check("resized primary: last pixel → virtual (1918,1078)",
      v == (1918, 1078), f"got {v}")

# ── the guard that stops a confident wrong answer ────────────────────────────
print("\n[out-of-desktop rejection]")

# Far outside every monitor — this is the shape of a hallucinated coordinate.
v = screens.capture_to_virtual(99999, 99999, LEFT_MON)
check("absurd coordinate is rejected, not clicked", v is None, f"got {v}")

# Empty geometry must not silently produce (0,0), which would be a real click.
check("empty geometry rejected", screens.capture_to_virtual(10, 10, {}) is None)

# ── geometry the model is told about must match reality ──────────────────────
print("\n[live machine geometry]")


def _norm(l, t, w, h):
    """Normalise monitor 0 (the bounding box) to (minx, miny, width, height)."""
    return l, t, w, h


if screens.available():
    left, top, w, h = screens.virtual_bounds()
    mons = screens.list_monitors()
    n = len(mons) - 1
    print(f"    detected {n} screen(s); virtual desktop "
          f"{w}x{h} at ({left},{top})")
    check("virtual bounds are non-empty", w > 0 and h > 0)

    # Every individual monitor must lie inside the reported bounding box. This
    # is the property that makes capture_to_virtual's bounds check meaningful;
    # if the box were wrong, valid clicks would be discarded.
    ok = True
    for m in mons[1:]:
        if not (left <= m["left"] and m["left"] + m["width"] <= left + w
                and top <= m["top"] and m["top"] + m["height"] <= top + h):
            ok = False
            print(f"    monitor {m['index']} {m} escapes {left},{top} {w}x{h}")
    check("every monitor lies inside the virtual desktop", ok)

    # A capture of each monitor must map its own centre back into that monitor.
    for m in mons[1:]:
        img, geom = screens.capture(m["index"], annotate=False,
                                    max_w=1920, max_h=1080)
        if img is None:
            check(f"monitor {m['index']} captured", False, "capture returned None")
            continue
        cx, cy = geom["image_w"] // 2, geom["image_h"] // 2
        pt = screens.capture_to_virtual(cx, cy, geom)
        expect = (m["left"] + m["width"] // 2, m["top"] + m["height"] // 2)
        near = pt is not None and abs(pt[0] - expect[0]) <= 2 and abs(pt[1] - expect[1]) <= 2
        check(f"monitor {m['index']} centre round-trips", near,
              f"got {pt}, expected ~{expect}")

    # The active-monitor resolver must return something that exists.
    act = screens.active_monitor()
    check("active_monitor() names a real screen", act == 0 or 1 <= act <= n,
          f"got {act}")

    # ── the assistant must not photograph itself ─────────────────────────────
    # The HUD is a normal foreground window titled "<name> — <version>". If it
    # counted as "the focused window", asking about a chess game on screen 1
    # would capture whichever display the HUD happens to be drawn on, and the
    # model would describe its own face. These check the exclusion — and,
    # importantly, check that it is not TOO eager.
    check("our own window is recognised (em dash)",
          screens._is_self_window("JARVIS \u2014 MARK LIV") is True)
    check("our own window is recognised (plain hyphen)",
          screens._is_self_window("JARVIS - MARK LIV") is True)
    check("case does not matter",
          screens._is_self_window("jarvis \u2014 mark liv") is True)
    check("leading whitespace is tolerated",
          screens._is_self_window("  JARVIS \u2014 MARK LIV") is True)

    # The regressions this anchoring exists to prevent. The first version
    # matched the name ANYWHERE and excluded a real VS Code window whose folder
    # happened to be called "jarvis" — so the assistant refused to look at the
    # window the user was actually working in.
    check("a VS Code window mentioning 'jarvis' is NOT ours",
          screens._is_self_window("TELEPHONY.md - jarvis - Visual Studio Code") is False)
    check("a chat about the project is NOT ours",
          screens._is_self_window("Mark-LIV: build an assistant like JARVIS") is False)
    check("the name must start the title, not merely appear",
          screens._is_self_window("Notes about JARVIS \u2014 draft") is False)
    check("an unrelated window is NOT ours",
          screens._is_self_window("Chess.com - Google Chrome") is False)
    check("an empty title is not ours", screens._is_self_window("") is False)
    check("the name alone (no dash) is not treated as ours",
          screens._is_self_window("jarvis") is False)
    check("self-window markers are non-empty",
          len(screens._self_window_markers()) >= 1)

    check("describe() mentions every screen",
          all(f"#{m['index']}" in screens.describe() for m in mons[1:])
          or n <= 1)

    # Capture of "all" must be at least as large as any single monitor — proof
    # that the picture really does contain both displays.
    if n > 1:
        img_all, geom_all = screens.capture_all(annotate=False)
        img_one, geom_one = screens.capture(1, annotate=False)
        check("capture_all covers >= one monitor",
              geom_all.get("capture_w", 0) >= geom_one.get("capture_w", 0))

    # Every documented target must resolve to a capture whose reported region
    # actually LIES INSIDE the monitor it names. That is the invariant that
    # matters: if the geometry said one screen while containing another, the
    # coordinate bridge would convert answers into clicks on the wrong display.
    mons_by_index = {m["index"]: m for m in mons[1:]}
    for target in ("active", "window", "all", "1", "2", "99"):
        img, geom = screens.capture_target(target)
        if img is None:
            check(f"target {target!r} captures something", False, "image is None")
            continue
        mon = geom.get("monitor")
        left = geom.get("origin_left")
        if mon == 0:
            # "all" must genuinely span every monitor.
            vl, vt, vw, vh = screens.virtual_bounds()
            inside = (left <= vl and geom.get("capture_w", 0) >= vw)
            check(f"target {target!r} spans the whole desktop", inside,
                  f"region {left} w={geom.get('capture_w')} vs desktop "
                  f"{vl} w={vw}")
            continue
        m = mons_by_index.get(mon)
        if m is None:
            check(f"target {target!r} names a real monitor", False, f"mon={mon}")
            continue
        # The captured region must sit within the monitor it claims.
        region_l = left
        region_r = left + geom.get("capture_w", 0)
        inside = (m["left"] <= region_l and region_r <= m["left"] + m["width"])
        check(f"target {target!r} region lies inside monitor {mon}", inside,
              f"region {region_l}..{region_r} vs monitor "
              f"{m['left']}..{m['left'] + m['width']}")

    # The numeric targets must select the monitor they name.
    for want in (1, 2):
        _, g = screens.capture_target(str(want))
        check(f"target {want!r} selects monitor {want}", g.get("monitor") == want,
              f"got {g.get('monitor')}")

    # ── the blank-capture trap ───────────────────────────────────────────────
    # pyautogui.screenshot(region=...) accepts a rectangle on a monitor to the
    # LEFT of the primary, returns the right SIZE, raises nothing — and returns
    # a totally black image. A crop of a window on that monitor would therefore
    # have shown the model a black screen, which it would have described in good
    # faith. These checks assert every region we can hand over contains picture.
    import numpy as _np

    def _mean_pixel(im) -> float:
        return float(_np.asarray(im.convert("L")).mean())

    for m in mons[1:]:
        img, geom = screens.grab_region(m["left"], m["top"],
                                        m["width"], m["height"],
                                        annotate=False,
                                        max_w=None, max_h=None)
        if img is None:
            check(f"region grab of monitor {m['index']}", False, "returned None")
            continue
        mn = _mean_pixel(img)
        check(f"region grab of monitor {m['index']} is not blank",
              mn > 1.0, f"mean pixel {mn:.2f} (a black frame)")

    # A small sub-region deep inside the negative-origin monitor must line up
    # with the equivalent crop of the whole monitor. This is what proves it is
    # POSITIONED correctly, not merely non-black.
    #
    # Compared with a tolerance, and against a deliberately WRONG offset as a
    # control. Byte-equality was the first attempt and it was flaky: the two
    # screenshots are taken a moment apart, so a ticking clock or a blinking
    # cursor changes a few pixels (measured 28.01 vs 27.99). Asserting "closer
    # to the right place than the wrong place" is both robust on a live desktop
    # and a stronger claim than equality.
    m_left = next((m for m in mons[1:] if m["left"] < 0), None)
    if m_left is not None:
        full, _ = screens.capture(m_left["index"], annotate=False,
                                  max_w=None, max_h=None)
        sub, _ = screens.grab_region(-800, 900, 300, 120, annotate=False,
                                     max_w=None, max_h=None)
        if full is not None and sub is not None:
            ox = -800 - m_left["left"]
            right = full.crop((ox, 900, ox + 300, 1020))
            wrong = full.crop((ox + 400, 900, ox + 700, 1020))

            def _dist(a, b) -> float:
                return float(_np.abs(_np.asarray(a, dtype=_np.int16)
                                     - _np.asarray(b, dtype=_np.int16)).mean())

            d_right = _dist(sub, right)
            d_wrong = _dist(sub, wrong)
            check("negative-origin sub-region matches the correct offset",
                  d_right < 6.0,
                  f"mean abs diff {d_right:.2f} at the right offset")
            check("negative-origin sub-region is clearly NOT the wrong offset",
                  d_wrong > d_right + 2.0,
                  f"right={d_right:.2f} wrong={d_wrong:.2f}")
else:
    print("    (mss unavailable — skipping live checks)")

# ── send_message: the outcome must be classifiable ───────────────────────────
print("\n[send_message honesty]")

import actions.send_message as sm  # noqa: E402

# The destructive sequence must be disabled.
check("_CLEAR_ALLOWED is False", sm._CLEAR_ALLOWED is False)

# Calling it must RAISE rather than delete — the whole point of the guard.
try:
    sm._clear_and_paste("x")
    check("_clear_and_paste refuses to run", False, "it ran without raising")
except RuntimeError as e:
    check("_clear_and_paste refuses to run", "Ctrl+A" in str(e))
except Exception as e:
    check("_clear_and_paste refuses to run", True, f"raised {type(e).__name__}")

# _replace_field must exist and must NOT press delete. Source is inspected
# rather than the function called, so this test cannot type into the machine it
# runs on.
src = (Path(sm.__file__)).read_text(encoding="utf-8")


import ast  # noqa: E402
import inspect as _inspect  # noqa: E402


def _code_of(fn_name: str) -> str:
    """The function's CODE, with comments and its docstring removed.

    This matters: the first version of these tests searched the raw source and
    failed because the prose explaining a bug quoted the buggy code. A check
    like "the old pattern is gone" has to run against executable text, or it
    matches its own explanation and can never pass.
    """
    tree = ast.parse(src)
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == fn_name:
            if (node.body and isinstance(node.body[0], ast.Expr)
                    and isinstance(node.body[0].value, ast.Constant)):
                node.body = node.body[1:]          # drop the docstring
            return ast.unparse(node)
    return ""


def _strip_docstrings(tree: ast.AST) -> ast.AST:
    """Remove EVERY docstring in the module, recursively.

    Not just the one function: the docstrings in this module are long, and they
    quote the exact lines being fixed. `ast.unparse` keeps docstrings as string
    literals, so a module-wide scan without this step still finds the buggy code
    — inside the explanation of why it was buggy.
    """
    for node in ast.walk(tree):
        if not isinstance(node, (ast.Module, ast.FunctionDef, ast.AsyncFunctionDef,
                                 ast.ClassDef)):
            continue
        body = getattr(node, "body", None)
        if (body and isinstance(body[0], ast.Expr)
                and isinstance(body[0].value, ast.Constant)
                and isinstance(body[0].value.value, str)):
            node.body = body[1:] or [ast.Pass()]
    return tree


rep = _code_of("_replace_field")
check("_replace_field presses no delete",
      rep != "" and "delete" not in rep.lower(), f"body={rep[:120]!r}")

send = _code_of("_desktop_send")
check("_desktop_send does not claim 'Message sent'",
      send != "" and "Message sent to" not in send)
check("_desktop_send verifies focus",
      "_focus_matches" in send)
check("_desktop_send reports unverified delivery",
      "not confirmed" in send.lower() or "NOT confirmed" in send)

# The circular check must be gone from EXECUTABLE code everywhere in the module.
code_only = ast.unparse(_strip_docstrings(ast.parse(src)))
check("no circular 'sent' substring check remains",
      "'sent' in result.lower()" not in code_only.replace(" ", ""))
check("no 'Message sent to' literal remains in code",
      "Message sent to" not in code_only)

# Every platform handler must route through the verified sender or say so.
for fn in ("_send_whatsapp", "_send_telegram", "_send_signal", "_send_discord"):
    b = _code_of(fn)
    check(f"{fn} routes through _desktop_send", "_desktop_send" in b, f"body={b!r}")

# The classifier must actually distinguish the three outcomes.
def classify(result: str) -> str:
    low = result.lower()
    failed = low.startswith(("refused", "could not", "error")) or "nothing was sent" in low
    sent = low.startswith("submitted") or "sent via" in low
    return "fail" if failed else ("sent" if sent else "info")


check("classifier: refusal → fail",
      classify("Refused: focus left Outlook — nothing was sent.") == "fail")
check("classifier: unreachable → fail",
      classify("Could not open Spotify — nothing was sent.") == "fail")
check("classifier: submitted → sent",
      classify("Submitted to Outlook for a@b.com — delivery is NOT confirmed.") == "sent")

# ── falsification: prove the checks can actually fail ────────────────────────
# A test that has never failed proves nothing. This deliberately re-introduces
# the original bug (deriving coordinates with no origin offset — exactly the old
# behaviour of treating the model's answer as absolute screen coordinates) and
# asserts that the invariant catches it.
print("\n[falsification — the checks must be able to fail]")


def _buggy_no_offset(mx, my, geom):
    """The old behaviour: treat the model's pixels as screen pixels."""
    return int(mx), int(my)


# A click derived this way looks plausible and is wrong by the whole monitor.
bad = _buggy_no_offset(960, 540, LEFT_MON)
good = screens.capture_to_virtual(960, 540, LEFT_MON)
check("the old no-offset arithmetic gives a DIFFERENT (wrong) answer",
      bad != good, f"bad={bad} good={good}")
check("the old arithmetic is wrong by exactly the monitor offset",
      good[0] - bad[0] == -1920, f"delta={good[0] - bad[0]}")
check("the old answer would have been clicked on the wrong monitor",
      screens.monitor_for_point(*bad) != screens.monitor_for_point(*good))

# And the circular success check must be shown to be unfalsifiable.
def _buggy_tick(result: str) -> str:
    return "\u2705" if "sent" in result.lower() else "\u274c"


check("the old tick reports success for a message that was never sent",
      _buggy_tick("Message sent to a@b.com via Outlook.") == "\u2705")
check("the new classifier does NOT",
      classify("Refused: focus left Outlook — nothing was sent.") == "fail")

# ── summary ──────────────────────────────────────────────────────────────────
print(f"\n{len(PASS)} passed, {len(FAIL)} failed")
if FAIL:
    print("FAILED:")
    for f in FAIL:
        print(f"  - {f}")
    sys.exit(1)
print("All invariants hold.")
