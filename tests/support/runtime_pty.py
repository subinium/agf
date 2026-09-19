"""Real AGF Unix PTY acceptance tests; no provider calls or production hooks."""

import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import signal
import sqlite3
import struct
import subprocess
import sys
import tempfile
import termios
import time
import uuid

import pyte


ALT_ON = b"\x1b[?1049h"
ALT_OFF = b"\x1b[?1049l"
PASTE_ON = b"\x1b[?2004h"
FRAME_END = b"\x1b[?2026l"
HANDOFF = b"AGF_PTY_SYNTHETIC_HANDOFF"
EXITED = b"AGF_PTY_CHILD_EXITED"
STEP_TIMEOUT = 8.0
OUTPUT_LIMIT = 2 * 1024 * 1024
# SLT 0.25's gray ramp is 232 + (channel - 8) // 10: 59 -> 237,
# 228 -> 254. The latter resolves to exactly #e4e4e4 in standard xterm-256.
HIGHLIGHT_BACKGROUNDS = {"3b3b3b", "3a3a3a", "e4e4e4"}
SELECTION_BACKGROUNDS = {"dark": "3b3b3b", "light": "e4e4e4",
                        "dark_256": "3a3a3a", "light_256": "e4e4e4",
                        "dark_basic": "black", "light_basic": "white"}
PAGE_BACKGROUNDS = {"dark": "181b1d", "light": "f5f7f7",
                   "dark_256": "000000", "light_256": "ffffff",
                   "dark_basic": "black", "light_basic": "white"}
BASIC_APPEARANCES = {"dark_basic", "light_basic"}
COLOR_ROLES = {
    "dark": {"selection_text": "fafafa", "selection_muted": "c1c1c1",
             "accent": "58cdcf", "danger": "f58b8b"},
    "light": {"selection_text": "202020", "selection_muted": "525252",
              "accent": "00686f", "danger": "ac2930"},
    "dark_256": {"selection_text": "ffffff", "selection_muted": "bcbcbc",
                 "accent": "5fd7d7", "danger": "ff8787"},
    "light_256": {"selection_text": "1c1c1c", "selection_muted": "4e4e4e",
                  "accent": "005f5f", "danger": "af0000"},
    "no_color": dict.fromkeys(("selection_text", "selection_muted", "accent", "danger"), "default"),
    "dark_basic": dict.fromkeys(("selection_text", "selection_muted", "accent", "danger"), "white"),
    "light_basic": dict.fromkeys(("selection_text", "selection_muted", "accent", "danger"), "black"),
}
SESSION_ID = "pty-id ' ; printf UNEXPECTED_EXECUTION ; $HOME"
CONVERSATION_ID = "c96a140c-d4c0-4996-9b9b-03a0468b1fcc"
PROVIDERS = ("claude", "codex", "grok", "kimi", "qwen", "opencode", "pi", "omp",
             "kiro-cli", "cursor-agent", "gemini", "hermes", "yolop", "prime-agent", "agy")
TYPED = "\ud55c\uae00"
PASTED = "\ubd99\uc5ec\ub123\uae30"
F1 = b"\x1bOP"
F2 = b"\x1bOQ"
F3 = b"\x1bOR"
F4 = b"\x1bOS"
UP = b"\x1b[A"
DOWN = b"\x1b[B"
RIGHT = b"\x1b[C"
LEFT = b"\x1b[D"
HOME = b"\x1b[H"
END = b"\x1b[F"
PAGE_UP = b"\x1b[5~"
PAGE_DOWN = b"\x1b[6~"
WHEEL_UP = b"\x1b[<64;5;5M"
WHEEL_DOWN = b"\x1b[<65;5;5M"
UI_CASES = {"literal_query_caret", "help_compact", "details_scroll", "footer_widths"}
FAILURE_CASES = {"scan_failure_status", "scan_failure_empty"}
THEME_CASES = {"theme_dark", "theme_light", "theme_dark_256", "theme_light_256",
               "theme_auto_light", "theme_no_color", "theme_settings",
               "theme_dark_basic", "theme_light_basic"}
WATCH_CASES = {"watch_dark", "watch_light"}


class SltByteStream(pyte.ByteStream):
    """Map only SLT's single underline SGR to pyte's equivalent legacy SGR."""

    def __init__(self, screen):
        super().__init__(screen)
        self._pending_csi = bytearray()
        self._escape = False
        self._control_string = None
        self._string_escape = False

    def feed(self, data):
        output = bytearray()
        for byte in data:
            if self._control_string is not None:
                output.append(byte)
                if ((self._control_string == 0x5d and byte == 7)
                        or (self._string_escape and byte == 0x5c)):
                    self._control_string = None
                self._string_escape = byte == 0x1b
            elif self._pending_csi:
                self._pending_csi.append(byte)
                if 0x40 <= byte <= 0x7e or len(self._pending_csi) >= 64:
                    sequence = bytes(self._pending_csi)
                    output.extend(b"\x1b[4m" if sequence == b"\x1b[4:1m" else sequence)
                    self._pending_csi.clear()
                elif byte == 0x1b:
                    output.extend(self._pending_csi[:-1])
                    self._pending_csi.clear()
                    self._escape = True
            elif self._escape:
                self._escape = False
                if byte == 0x5b:
                    self._pending_csi.extend(b"\x1b[")
                elif byte == 0x1b:
                    output.append(0x1b)
                    self._escape = True
                else:
                    output.extend((0x1b, byte))
                    if byte in b"]PX^_":
                        self._control_string = byte
                        self._string_escape = False
            elif byte == 0x1b:
                self._escape = True
            else:
                output.append(byte)
        super().feed(bytes(output))


class FrameScreen(pyte.Screen):
    """Let pyte interpret VT bytes; publish only completed synchronized frames."""

    def __init__(self, columns, lines):
        self.frame_serial = 0
        self.geometry_epoch = 0
        self.completed = None
        self.completed_highlights = ()
        self.completed_cells = ()
        self._frame_open = False
        self._frame_epoch = 0
        self._needs_redraw = False
        super().__init__(columns, lines)

    def set_mode(self, *modes, **kwargs):
        super().set_mode(*modes, **kwargs)
        if kwargs.get("private") and 2026 in modes:
            self.frame_serial += 1
            self._frame_open = True
            self._frame_epoch = self.geometry_epoch

    def reset_mode(self, *modes, **kwargs):
        super().reset_mode(*modes, **kwargs)
        if kwargs.get("private") and 2026 in modes and self._frame_open:
            self._frame_open = False
            if not self._needs_redraw and self._frame_epoch == self.geometry_epoch:
                self.completed = (self.frame_serial, self.geometry_epoch,
                                  (self.columns, self.lines), "\n".join(self.display))
                self.completed_cells = tuple(
                    tuple(self.buffer[y][x] for x in range(self.columns))
                    for y in range(self.lines)
                )
                self.completed_highlights = tuple(
                    line for y, line in enumerate(self.display)
                    if any(cell.bg in HIGHLIGHT_BACKGROUNDS for cell in self.completed_cells[y])
                )

    def resize(self, lines=None, columns=None):
        previous = (self.columns, self.lines)
        super().resize(lines=lines, columns=columns)
        if (self.columns, self.lines) != previous:
            self.geometry_epoch += 1
            self.completed = None
            self.completed_highlights = ()
            self.completed_cells = ()
            self._needs_redraw = True

    def erase_in_display(self, how=0, *args, **kwargs):
        super().erase_in_display(how, *args, **kwargs)
        if how in (2, 3):
            self._needs_redraw = False

    def matches(self, anchors, after_serial):
        if self.completed is None:
            return False
        serial, epoch, geometry, text = self.completed
        return (serial > after_serial and epoch == self.geometry_epoch
                and geometry == (self.columns, self.lines)
                and all(anchor in text for anchor in anchors))

    def is_selected(self, label, after_serial):
        return (self.matches([label], after_serial)
                and any(label in line for line in self.completed_highlights))

    def is_marked(self, label, after_serial):
        return (self.matches([label], after_serial)
                and any(line.lstrip().startswith(">") and label in line
                        for line in self.completed[3].splitlines()))

    def footer_matches(self, keys, after_serial):
        if not self.matches([], after_serial):
            return False
        footer = self.completed[3].splitlines()[-1]
        return all(re.search(r"(?<![A-Za-z0-9])" + re.escape(key) + r"(?![A-Za-z0-9])",
                             footer) for key in keys)


def terminal_attributes(fd):
    attributes = termios.tcgetattr(fd)
    attributes[6] = [value[0] if isinstance(value, bytes) else value for value in attributes[6]]
    return attributes


def fd_flags(fd):
    return fcntl.fcntl(fd, fcntl.F_GETFL)


def visible_bytes(data):
    data = re.sub(rb"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)", b"", data)
    data = re.sub(rb"\x1bP.*?\x1b\\", b"", data, flags=re.S)
    data = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", data)
    data = re.sub(rb"\x1b[=>78]", b"", data)
    return data.replace(b"\r", b"").decode("utf-8", errors="replace")


def modes_at(data):
    modes = {}
    for match in re.finditer(rb"\x1b\[\?([0-9;]+)([hl])", data):
        for mode in match[1].split(b";"):
            modes[int(mode)] = match[2] == b"h"
    return modes


def check_cleanup(data):
    modes = modes_at(data)
    assert ALT_ON in data and ALT_OFF in data, "alternate screen must be entered and left"
    assert PASTE_ON in data, "bracketed paste was not enabled"
    for mode in (1049, 2004, 1000, 1002, 1003, 1006, 1015, 1004):
        assert modes.get(mode, False) is False, "terminal mode {} still enabled".format(mode)
    assert modes.get(25) is True, "cursor visibility was not restored"


def color_contrast(first, second):
    def luminance(color):
        assert re.fullmatch(r"[0-9a-f]{6}", color), "expected resolved RGB, got " + color
        channels = [int(color[index:index + 2], 16) / 255 for index in (0, 2, 4)]
        channels = [value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4
                    for value in channels]
        return sum(value * weight for value, weight in zip(channels, (0.2126, 0.7152, 0.0722)))

    light, dark = sorted((luminance(first), luminance(second)), reverse=True)
    return (light + 0.05) / (dark + 0.05)


def assert_no_color(data):
    for sgr in re.findall(rb"\x1b\[([0-9;:]*)m", data):
        assert not re.search(rb"(?:^|;)(?:3[0-8]|4[0-8]|9[0-7]|10[0-7])(?:[;:]|$)", sgr), \
            "NO_COLOR emitted a foreground/background SGR: {!r}".format(sgr)


def is_neutral(color):
    return color in ("black", "white") or bool(re.fullmatch(r"([0-9a-f]{2})\1\1", color))


def assert_basic_palette(screen, appearance, output):
    assert appearance in BASIC_APPEARANCES
    foreground = COLOR_ROLES[appearance]["selection_text"]
    background = PAGE_BACKGROUNDS[appearance]
    assert foreground != background
    for y, row in enumerate(screen.completed_cells):
        for x, cell in enumerate(row):
            assert cell.bg == background, "Basic background mismatch at ({}, {}): {}".format(x, y, cell)
            if cell.data.strip():
                assert cell.fg == foreground, "Basic glyph must use plain {}: {}".format(foreground, cell)
    for sgr in re.findall(rb"\x1b\[([0-9;:]*)m", output):
        assert not re.search(rb"(?:^|;)(?:3[1-6]|4[1-6]|9[0-7]|10[0-7]|38|48)(?:[;:]|$)", sgr), \
            "Basic fallback emitted an approximate hue or extended color SGR: {!r}".format(sgr)


def assert_marker_roles(screen, appearance):
    assert screen.matches([], 0), "color roles require a completed current-geometry frame"
    counts = {"pointer": 0, "pin": 0, "menu_number": 0}
    for y, (line, cells) in enumerate(zip(screen.completed[3].splitlines(), screen.completed_cells)):
        spans = [(match.start(), match.end(), "pointer") for match in re.finditer(">", line)]
        pin = re.match(r"^[ >\u2514\u251c\u2500]*\*", line)
        if pin:
            spans.append((pin.end() - 1, pin.end(), "pin"))
        number = re.match(r"^\s*>?\s*(\d+\))", line)
        if number:
            spans.append((*number.span(1), "menu_number"))
        selected = line.lstrip().startswith(">")
        expected = COLOR_ROLES[appearance]["selection_text" if selected else "selection_muted"]
        for start, end, role in spans:
            counts[role] += 1
            for x in range(start, end):
                cell = cells[x]
                assert cell.fg == expected, "{} at ({}, {}) must use neutral {}, got {}".format(
                    role, x, y, expected, cell)
                assert appearance == "no_color" or is_neutral(cell.fg), "marker retained a brand hue"
    assert counts["pointer"], "frame must exercise a visible pointer"
    return counts


def assert_footer_accent(screen, appearance):
    cells = [cell for cell in screen.completed_cells[-1] if cell.data.strip() and cell.bold]
    assert cells, "footer must retain bold action keys"
    assert all(cell.fg == COLOR_ROLES[appearance]["accent"] for cell in cells), \
        "neutral markers must not remove the footer's action accent"


def assert_grouped_roles(screen, appearance, brands):
    checked = set()
    for line, cells in zip(screen.completed[3].splitlines(), screen.completed_cells):
        match = re.match(r"^\s*>?\s*[\u2514\u251c]\u2500[ *]+(claude|codex)\s+", line)
        if not match:
            continue
        agent = match[1]
        checked.add(agent)
        start, end = match.span(1)
        assert all(cell.fg == brands[agent] for cell in cells[start:end]), \
            "grouped agent label lost its brand: " + agent
        summary_and_time = [cell for cell in cells[match.end():] if cell.data.strip()]
        assert summary_and_time, "grouped fixture must contain summary and timestamp glyphs"
        for x, cell in enumerate(cells):
            if cell.data.strip() and not start <= x < end:
                assert (cell.fg == "default" if appearance == "no_color" else is_neutral(cell.fg)), \
                    "grouped tree/summary/time must be exact neutral outside {} label at {}: {}".format(agent, x, cell)
                if appearance != "no_color" and appearance not in BASIC_APPEARANCES:
                    assert cell.fg != brands["codex"], "neutral group content conflicts with Codex"
    assert checked, "expected visible grouped children"
    return checked


class Fixture:
    def __init__(self, antigravity=False, all_providers=False, ui_polish=False,
                 failed_provider=False, empty=False, unique_query=False, appearance=None,
                 codex_brand=False):
        self.antigravity = antigravity
        self.all_providers = all_providers
        self.ui_polish = ui_polish
        self.failed_provider = failed_provider
        self.empty = empty
        self.unique_query = unique_query
        self.appearance = appearance
        self.codex_brand = codex_brand
        self.session_count = 0 if empty else 20 if all_providers else 3 if ui_polish else 1

    def __enter__(self):
        # Numeric path components cannot accidentally match the Unicode queries.
        self.root = Path(tempfile.gettempdir()).resolve() / str(uuid.uuid4().int)
        self.root.mkdir(mode=0o700)
        self.home = self.root / "1001"
        self.launch = self.root / "2001"
        self.project = self.root / "3001 ' $HOME"
        if self.ui_polish:
            self.project /= "PTY_LITERAL_?[]"
        self.bin = self.launch / "4001"
        self.storage = self.launch / "5001"
        self.capture = self.root / "7001"
        self.exit_capture = self.root / "7002"
        for directory in (self.home, self.launch, self.project, self.bin, self.storage / "projects" / "6001"):
            directory.mkdir(parents=True, exist_ok=True)
        config_root = (self.home / "Library" / "Application Support"
                       if sys.platform == "darwin" else self.home / "config")
        self.config = config_root / "agf" / "config.toml"
        if self.appearance in ("light", "light_256", "light_basic"):
            self.config.parent.mkdir(parents=True)
            self.config.write_text('appearance = "light"\n', encoding="utf-8")
        self.history = self.storage / "history.jsonl"
        self.transcript = self.storage / "projects" / "6001" / (SESSION_ID + ".jsonl")
        self.history.write_text(json.dumps({
            "display": "PTY_FIXTURE_ONLY",
            "timestamp": int(time.time() * 1000),
            "project": str(self.project),
            "sessionId": SESSION_ID,
        }) + "\n", encoding="utf-8")
        self.transcript.write_text(json.dumps({
            "type": "user", "cwd": str(self.project),
            "message": {"role": "user", "content": "PTY_FIXTURE_ONLY"},
        }) + "\n", encoding="utf-8")
        self.provider_roots = [self.storage]
        if self.antigravity:
            self.history.unlink()
            self.transcript.unlink()
            self.agy_storage = self.home / ".gemini" / "antigravity-cli"
            self.agy_storage.mkdir(parents=True)
            database = self.agy_storage / "conversation_summaries.db"
            connection = sqlite3.connect(database)
            try:
                connection.execute("""CREATE TABLE conversation_summaries (
                    conversation_id TEXT PRIMARY KEY, title TEXT, preview TEXT,
                    workspace_uris TEXT, last_modified_time TEXT,
                    parent_conversation_id TEXT, nesting_depth INTEGER)""")
                connection.execute("INSERT INTO conversation_summaries VALUES (?, ?, ?, ?, ?, ?, ?)",
                                   (CONVERSATION_ID, "PTY_FIXTURE_ONLY", "PTY_FIXTURE_ONLY",
                                    json.dumps([self.project.as_uri()]), "2026-09-18T05:31:00Z", "", 0))
                connection.commit()
            finally:
                connection.close()
            transcript = (self.agy_storage / "brain" / CONVERSATION_ID /
                          ".system_generated" / "logs" / "transcript.jsonl")
            transcript.parent.mkdir(parents=True)
            transcript.write_text(json.dumps({"type": "USER_INPUT", "content": "PTY_FIXTURE_ONLY",
                                              "created_at": "2026-09-18T05:31:00Z"}) + "\n",
                                  encoding="utf-8")
            self.provider_roots.append(self.agy_storage)
        elif self.all_providers:
            with self.history.open("a", encoding="utf-8") as history:
                for i in range(1, self.session_count):
                    project = self.project / "PTY_ROW_{:02}".format(i)
                    if self.unique_query and i == self.session_count - 1:
                        project = project.with_name(project.name + "_UNIQUE_NEEDLE")
                    project.mkdir()
                    transcript = self.storage / "projects" / "6001" / "pty-row-{:02}.jsonl".format(i)
                    transcript.write_text(json.dumps({"type": "user", "cwd": str(project),
                                                      "message": {"role": "user", "content": "PTY_ROW_{:02}".format(i)}}) + "\n",
                                          encoding="utf-8")
                    history.write(json.dumps({"display": "PTY_ROW_{:02}".format(i),
                                              "timestamp": int(time.time() * 1000) - i * 10000,
                                              "project": str(project),
                                              "sessionId": "pty-row-{:02}".format(i)}) + "\n")
        if self.ui_polish:
            with self.history.open("a", encoding="utf-8") as history:
                for i in range(1, 10):
                    summary = "DETAIL_{:02} {} DETAILS_END_{:02}".format(
                        i, "wrapped-content " * 35, i)
                    history.write(json.dumps({"display": summary,
                                              "timestamp": int(time.time() * 1000) - i * 10000,
                                              "project": str(self.project),
                                              "sessionId": SESSION_ID}) + "\n")
                for i in range(1, self.session_count):
                    project = self.project.parent / "PTY_OTHER_{:02}".format(i)
                    project.mkdir()
                    if self.codex_brand and i == 2:
                        codex_root = self.home / ".codex"
                        rollout = codex_root / "sessions" / "rollout-pty-codex.jsonl"
                        rollout.parent.mkdir(parents=True)
                        timestamp = int(time.time()) - 200
                        rollout.write_text(json.dumps({"type": "session_meta", "payload": {
                            "id": "pty-codex-only", "cwd": str(project),
                            "timestamp": "2020-01-01T00:00:00Z"}}) + "\n", encoding="utf-8")
                        os.utime(rollout, (timestamp, timestamp))
                        (codex_root / "history.jsonl").write_text(json.dumps({
                            "session_id": "pty-codex-only", "ts": timestamp,
                            "text": "PTY_CODEX_SUMMARY"}) + "\n", encoding="utf-8")
                        self.provider_roots.append(codex_root)
                        continue
                    session_id = "pty-other-{:02}".format(i)
                    transcript = self.storage / "projects" / "6001" / (session_id + ".jsonl")
                    transcript.write_text(json.dumps({"type": "user", "cwd": str(project),
                                                      "message": {"role": "user", "content": project.name}}) + "\n",
                                          encoding="utf-8")
                    history.write(json.dumps({"display": project.name,
                                              "timestamp": int(time.time() * 1000) - i * 100000,
                                              "project": str(project),
                                              "sessionId": session_id}) + "\n")
        if self.empty:
            self.history.unlink()
            self.transcript.unlink()
        if self.failed_provider:
            bad_storage = self.home / ".gemini" / "antigravity-cli"
            bad_storage.mkdir(parents=True)
            (bad_storage / "conversation_summaries.db").write_bytes(b"AGF_PTY_INVALID_SQLITE_ONLY")
            self.provider_roots.append(bad_storage)
        self.original_files = self.provider_files()
        helper = str(Path(__file__).resolve())
        providers = (PROVIDERS if self.all_providers else ("agy",) if self.antigravity
                     else ("claude", "agy") if self.failed_provider
                     else ("claude", "codex") if self.codex_brand else ("claude",))
        for provider in providers:
            stub = "#!/bin/sh\nAGF_PTY_PROVIDER={} exec {} -I -X utf8 {} fake \"$@\"\n".format(
                provider, shlex.quote(sys.executable), shlex.quote(helper)
            )
            (self.bin / provider).write_text(stub, encoding="utf-8")
            (self.bin / provider).chmod(0o700)
        # Only the explicit system shell and our fake provider are on PATH.
        (self.bin / "sh").symlink_to("/bin/sh")
        return self

    def environment(self, ack_fd):
        environment = {
            "HOME": str(self.home), "USERPROFILE": str(self.home),
            "XDG_CONFIG_HOME": str(self.home / "config"),
            "XDG_DATA_HOME": str(self.home / "data"),
            "XDG_CACHE_HOME": str(self.home / "cache"),
            "APPDATA": str(self.home / "appdata"),
            "LOCALAPPDATA": str(self.home / "localappdata"),
            "TMPDIR": str(self.root), "PATH": "4001",
            "SHELL": "/bin/sh", "AGF_SHELL": "posix",
            "TERM": "xterm-256color", "LANG": "C.UTF-8", "TZ": "UTC",
            "CLAUDE_CONFIG_DIR": "5001",
            # These variables are consumed solely by the synthetic CLI below.
            "AGF_PTY_CAPTURE": str(self.capture), "AGF_PTY_ACK_FD": str(ack_fd),
            "AGF_PTY_EXIT_CAPTURE": str(self.exit_capture),
        }
        if self.appearance in BASIC_APPEARANCES:
            environment["TERM"] = "vt100"
        elif self.appearance is not None and not self.appearance.endswith("_256"):
            environment["COLORTERM"] = "truecolor"
        if self.appearance in ("light", "light_256", "light_basic"):
            environment["COLORFGBG"] = "15;0"
        elif self.appearance in ("auto_light", "no_color"):
            environment["COLORFGBG"] = "0;15"
        if self.appearance == "no_color":
            environment["NO_COLOR"] = "1"
        return environment

    def provider_files(self):
        return {str(path.relative_to(self.root)): path.read_bytes()
                for root in self.provider_roots for path in root.rglob("*") if path.is_file()}

    def assert_history_unchanged(self):
        assert self.provider_files() == self.original_files, "TUI added, removed, or mutated provider store bytes"

    def __exit__(self, *_):
        shutil.rmtree(self.root)


class PtyRun:
    def __init__(self, executable, fixture, arguments=(), prime_stdout=True):
        self.fixture = fixture
        self.output = bytearray()
        self.phase = "startup"
        self.protocol_replies = 0
        self.background_replies = 0
        self.answered = set()
        self.screen = FrameScreen(96, 24)
        self.stream = SltByteStream(self.screen)
        self.master, self.slave = pty.openpty()
        name = os.ttyname(self.slave)
        # Separate open descriptions catch stdin and stdout flag leakage independently.
        self.input_fd = os.open(name, os.O_RDONLY | os.O_NOCTTY)
        self.output_fd = os.open(name, os.O_WRONLY | os.O_NOCTTY)
        self.ack_read, self.ack_write = os.pipe()
        # Darwin exposes FWASWRITTEN after the first write. Prime the fixture
        # before its baseline; compare every flag exactly rather than mask bits.
        # https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/fcntl.h
        if prime_stdout:
            os.write(self.output_fd, b"\r")
        self.original_termios = terminal_attributes(self.slave)
        self.original_flags = [fd_flags(self.input_fd), fd_flags(self.output_fd)]
        cooked = termios.ICANON | termios.ECHO | termios.ISIG
        assert self.original_termios[3] & cooked == cooked, "PTY must start cooked"
        self.set_size(96, 24)
        os.set_blocking(self.master, False)

        def acquire_terminal():
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)

        self.process = subprocess.Popen(
            [sys.executable, "-I", "-X", "utf8", str(Path(__file__).resolve()), "supervise", str(executable), *arguments],
            cwd=str(fixture.launch), env=fixture.environment(self.ack_read),
            stdin=self.input_fd, stdout=self.output_fd, stderr=self.output_fd,
            start_new_session=True, preexec_fn=acquire_terminal,
            pass_fds=(self.ack_read,), close_fds=True,
        )

    def __enter__(self):
        return self

    def __exit__(self, error_type, *_):
        if error_type is not None:
            completed = self.screen.completed
            print("PTY failure phase: {} (pid {})\nCompleted screen {}:\n{}\nVisible tail: {}\nControl tail: {!r}".format(
                self.phase, self.process.pid, completed[:3] if completed else None,
                completed[3] if completed else "<no completed frame>",
                visible_bytes(bytes(self.output))[-1500:], bytes(self.output[-500:])), file=sys.stderr)
        if self.process.poll() is None:
            # Failure cleanup, never a rescue key or a successful test outcome.
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for fd in (self.master, self.slave, self.input_fd, self.output_fd, self.ack_read, self.ack_write):
            os.close(fd)
        self.process.wait(timeout=STEP_TIMEOUT)

    def set_size(self, columns, rows):
        self.columns, self.rows = columns, rows
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        self.screen.resize(lines=rows, columns=columns)

    def send(self, data):
        assert os.write(self.master, data) == len(data), "short PTY input write"

    def reply_to_queries(self):
        # These are terminal protocol acknowledgements, never user/rescue keystrokes.
        background = (b"ffff/ffff/ffff" if self.fixture.appearance in ("light", "light_256", "light_basic", "auto_light", "no_color")
                      else b"0000/0000/0000")
        background_query = rb"\x1b\]11;\?(?:\x07|\x1b\\)"
        replies = [
            (rb"\x1b\[6n", b"\x1b[1;1R"),
            (rb"\x1b\[(?:0)?c", b"\x1b[?1;2c"),
            (rb"\x1b\[>c", b"\x1b[>0;0;0c"),
            (rb"\x1b\[\?u", b"\x1b[?0u"),
            (rb"\x1b\[\?2026\$p", b"\x1b[?2026;2$y"),
            (background_query, b"\x1b]11;rgb:" + background + b"\x1b\\"),
            (rb"\x1b\[14t", "\x1b[4;{};{}t".format(self.rows * 16, self.columns * 8).encode()),
            (rb"\x1b\[16t", b"\x1b[6;16;8t"),
        ]
        for pattern, response in replies:
            for match in re.finditer(pattern, self.output):
                key = (pattern, match.start())
                if key not in self.answered:
                    self.answered.add(key)
                    self.send(response)
                    self.protocol_replies += 1
                    if pattern == background_query:
                        self.background_replies += 1

    def wait(self, predicate, phase):
        self.phase = phase
        deadline = time.monotonic() + STEP_TIMEOUT
        while not predicate():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise AssertionError("{} timed out; exit={}".format(phase, self.process.poll()))
            readable, _, _ = select.select([self.master], [], [], remaining)
            if not readable:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                chunk = b""
            if not chunk:
                raise AssertionError("PTY closed during " + phase)
            self.output.extend(chunk)
            assert len(self.output) <= OUTPUT_LIMIT, "unexpected unbounded terminal output"
            self.stream.feed(chunk)
            self.reply_to_queries()

    def completed_output(self, after):
        segment = bytes(self.output[after:])
        end = segment.rfind(FRAME_END)
        return segment[:end + len(FRAME_END)] if end >= 0 else b""

    def mark_frame(self):
        return len(self.output), self.screen.frame_serial

    def frame(self, anchors, after, phase):
        offset, serial = (0, 0) if after == 0 else after
        self.wait(lambda: self.screen.matches(anchors, serial)
                  and bool(self.completed_output(offset)), phase)
        return self.completed_output(offset)

    def selected(self, label, after, phase):
        _, serial = after
        self.wait(lambda: self.screen.is_selected(label, serial), phase)

    def input_frame(self, data, anchors, phase):
        after = self.mark_frame()
        self.send(data)
        self.frame(anchors, after, phase)

    def selected_input(self, data, label, phase):
        after = self.mark_frame()
        self.send(data)
        self.selected(label, after, phase)

    def marked_input(self, data, label, phase):
        after = self.mark_frame()
        self.send(data)
        self.wait(lambda: self.screen.is_marked(label, after[1]), phase)

    def assert_absent(self, text):
        assert self.screen.completed is not None
        assert text not in self.screen.completed[3], "unexpected visible text: " + text

    def footer(self, keys):
        assert self.screen.footer_matches(keys, 0), "missing whole footer keys {} at {}x{}:\n{}".format(
            keys, self.columns, self.rows, self.screen.completed[3])

    def theme_selection(self, appearance, label=""):
        if appearance != "no_color":
            background = SELECTION_BACKGROUNDS[appearance]

            def palette_ready():
                if not self.screen.is_marked(label, 0):
                    return False
                return any(line.lstrip().startswith(">") and label in line
                           and all(cell.bg == background for cell in cells if cell.data.strip())
                           for line, cells in zip(self.screen.completed[3].splitlines(), self.screen.completed_cells))

            # Settings input is processed after the frame's palette is chosen.
            # Observe the next completed styled frame, without injecting input.
            self.wait(palette_ready, "completed {} palette for {}".format(appearance, label))
        assert self.screen.is_marked(label, 0), "selected row lost its leading > marker: " + label
        rows = [(line, cells) for line, cells in
                zip(self.screen.completed[3].splitlines(), self.screen.completed_cells)
                if line.lstrip().startswith(">")]
        assert len(rows) == 1, "expected one selected row, got {}".format(rows)
        line, cells = rows[0]
        assert label in line
        assert_marker_roles(self.screen, appearance)
        assert_footer_accent(self.screen, appearance)
        if appearance == "no_color":
            assert all(cell.fg == cell.bg == "default"
                       for row in self.screen.completed_cells for cell in row), \
                "NO_COLOR retained colored cells"
            assert_no_color(bytes(self.output))
            return line
        if appearance in BASIC_APPEARANCES:
            assert_basic_palette(self.screen, appearance, bytes(self.output))
            assert not self.screen.is_selected(label, 0), "Basic selection must not pretend to have an RGB highlight"
            return line
        for y, row in enumerate(self.screen.completed_cells):
            for x, cell in enumerate(row):
                assert cell.bg in (PAGE_BACKGROUNDS[appearance], background), \
                    "palette background lost at ({}, {}): {}".format(x, y, cell)
        assert self.screen.is_selected(label, 0), "known palette selection background missing"
        glyphs = [cell for cell in cells if cell.data.strip()]
        assert glyphs
        for cell in glyphs:
            assert cell.bg == background, "unexpected selected-cell background: {}".format(cell)
            assert color_contrast(cell.fg, cell.bg) >= 4.5, \
                "selected glyph contrast below 4.5: {}".format(cell)
        return line

    def theme_brands(self, appearance):
        brands = {}
        for line, cells in zip(self.screen.completed[3].splitlines(), self.screen.completed_cells):
            match = re.match(r"^[ >*?]+(Claude Code|Codex)\s", line)
            if match:
                colors = {cell.fg for cell in cells[match.start(1):match.end(1)]}
                assert len(colors) == 1, "agent label must have one brand color"
                brands["claude" if match[1] == "Claude Code" else "codex"] = colors.pop()
        assert set(brands) == {"claude", "codex"}, "both isolated providers must be visible"
        if appearance == "no_color":
            assert set(brands.values()) == {"default"}
        elif appearance in BASIC_APPEARANCES:
            assert set(brands.values()) == {COLOR_ROLES[appearance]["selection_text"]}
        else:
            assert len(set(brands.values())) == 2, "Claude and Codex lost their distinct brand colors"
            assert all(not is_neutral(color) for color in brands.values()), "agent labels became gray"
        return brands

    def theme_token(self, appearance, token, role, selected=None):
        matches = []
        for line, cells in zip(self.screen.completed[3].splitlines(), self.screen.completed_cells):
            if selected is not None and line.lstrip().startswith(">") != selected:
                continue
            for match in re.finditer(re.escape(token), line):
                matches.extend(cells[match.start():match.end()])
        assert matches, "missing role probe: " + token
        assert all(cell.fg == COLOR_ROLES[appearance][role] for cell in matches), \
            "{} lost its {} color: {}".format(token, role, matches)

    def theme_snapshot(self, view):
        if not self.capture_cells:
            return
        assert self.screen.matches([], 0)
        self.theme_frames[view] = {
            "columns": self.columns, "rows": self.rows,
            "frame_serial": self.screen.completed[0], "text": self.screen.completed[3],
            "cells": [[cell._asdict() for cell in row] for row in self.screen.completed_cells],
        }

    def changed_content(self, data, phase):
        before = self.screen.completed[3].splitlines()[3:-2]
        after = self.mark_frame()
        self.send(data)
        self.wait(lambda: self.screen.matches([], after[1])
                  and self.screen.completed[3].splitlines()[3:-2] != before, phase)

    def search(self, data, query, count, phase, caret=None):
        after = self.mark_frame()
        self.send(data)
        caret = len(query) if caret is None else caret
        rendered = query[:caret] + "\u258e" + query[caret:]
        self.wait(lambda: self.screen.matches([count], after[1])
                  and rendered in self.screen.completed[3].splitlines()[1], phase)

    def setting_input(self, data, label, value, phase):
        after = self.mark_frame()
        self.send(data)

        def matches():
            if not self.screen.is_selected(label, after[1]):
                return False
            lines = self.screen.completed[3].splitlines()
            return any(label in line and lines[index + 1].strip() == value
                       for index, line in enumerate(lines[:-1]))

        self.wait(matches, phase)

    def initial(self, anchors=None):
        count = self.fixture.session_count
        body = "PTY_FIXTURE_ONLY" if count else "Sessions unavailable"
        self.frame(anchors if anchors is not None else ["All ({})".format(count), "{0}/{0}".format(count), body],
                   0, "initial populated render")
        assert ALT_ON in self.output and PASTE_ON in self.output
        assert not terminal_attributes(self.slave)[3] & (termios.ICANON | termios.ECHO), "TUI did not acquire raw input"
        assert not self.fixture.capture.exists(), "provider ran before a resume action"
        assert [fd_flags(self.input_fd), fd_flags(self.output_fd)] == self.original_flags, "active TUI changed stdin/stdout flags"

    def resize(self, columns, rows, anchors, rule=True):
        after = self.mark_frame()
        self.set_size(columns, rows)
        os.killpg(self.process.pid, signal.SIGWINCH)
        phase = "SIGWINCH {}x{} render".format(columns, rows)
        # Old redraws can arrive after ioctl and wrap into a full-width rule when
        # shrinking. Require both the visible rule and in-bounds addressing.
        def resized_frame_ready():
            if not self.screen.matches(anchors, after[1]):
                return False
            segment = self.completed_output(after[0])
            cleared = segment.rfind(b"\x1b[2J")
            if cleared < 0:
                return False
            positions = [(int(row), int(col)) for row, col in
                         re.findall(rb"\x1b\[(\d+);(\d+)H", segment[cleared:])]
            return (bool(positions)
                    and all(1 <= row <= rows and 1 <= col <= columns for row, col in positions)
                    and (not rule or "\u2500" * columns in self.screen.completed[3].splitlines()))

        self.wait(resized_frame_ready, phase)
        segment = self.completed_output(after[0])
        cleared = segment.rfind(b"\x1b[2J")
        assert cleared >= 0, "resize did not redraw the terminal"
        positions = [(int(row), int(col)) for row, col in re.findall(rb"\x1b\[(\d+);(\d+)H", segment[cleared:])]
        assert positions, "resize did not position the terminal cursor"
        if rule:
            assert any(line == "\u2500" * columns for line in self.screen.completed[3].splitlines()), "new terminal width was not rendered"
        assert all(1 <= row <= rows and 1 <= col <= columns for row, col in positions), "render addressed cells outside resized viewport"

    def assert_restored(self):
        assert terminal_attributes(self.slave) == self.original_termios, "termios leaked across TUI exit"
        assert [fd_flags(self.input_fd), fd_flags(self.output_fd)] == self.original_flags, "stdin/stdout flags leaked across TUI exit"
        check_cleanup(bytes(self.output))

    def finish(self):
        # Darwin revokes the slave when its controlling-session leader exits.
        # Keep the test supervisor alive until post-AGF terminal state is checked.
        self.wait(lambda: EXITED in self.output, "AGF child exit acknowledgement")
        record = json.loads(self.fixture.exit_capture.read_text(encoding="utf-8"))
        assert record["returncode"] == 0, record
        assert record["termios"] == self.original_termios, "termios leaked at AGF child exit"
        assert record["before_flags"] == self.original_flags, "supervisor changed initial fd flags: {}".format(record)
        assert [record["stdin_flags"], record["stdout_flags"]] == self.original_flags, "fd flags leaked at AGF child exit: before={}, after={}".format(self.original_flags, record)
        self.assert_restored()
        self.fixture.assert_history_unchanged()
        self.acknowledge_exit()

    def acknowledge_exit(self):
        assert os.write(self.ack_write, b"2") == 1
        self.process.wait(timeout=STEP_TIMEOUT)
        assert self.process.returncode == 0, "AGF/native handoff exited unsuccessfully"

    def stdio_control(self):
        self.wait(lambda: EXITED in self.output, "same-binary --version stdio control")
        record = json.loads(self.fixture.exit_capture.read_text(encoding="utf-8"))
        assert record["returncode"] == 0, record
        assert record["before_flags"] == self.original_flags, record
        assert record["termios"] == self.original_termios, "--version changed initial termios"
        assert terminal_attributes(self.slave) == self.original_termios
        assert ALT_ON not in self.output and PASTE_ON not in self.output
        text = visible_bytes(bytes(self.output))
        assert re.search(r"agf \d+\.\d+\.\d+", text), "stdio control did not execute agf --version"
        flags = [record["stdin_flags"], record["stdout_flags"]]
        assert [fd_flags(self.input_fd), fd_flags(self.output_fd)] == flags
        self.acknowledge_exit()
        return {"before_flags": self.original_flags, "after_flags": flags,
                "termios": self.original_termios, "version": text.splitlines()[0]}


def supervise(executable, arguments):
    ack = int(os.environ["AGF_PTY_ACK_FD"])
    before_flags = [fd_flags(0), fd_flags(1)]
    process = subprocess.Popen([executable, *arguments], pass_fds=(ack,))
    code = process.wait()
    record = {"returncode": code, "termios": terminal_attributes(0),
              "stdin_flags": fd_flags(0), "stdout_flags": fd_flags(1), "before_flags": before_flags}
    Path(os.environ["AGF_PTY_EXIT_CAPTURE"]).write_text(json.dumps(record), encoding="utf-8")
    print(EXITED.decode(), flush=True)
    readable, _, _ = select.select([ack], [], [], STEP_TIMEOUT)
    assert readable and os.read(ack, 1) == b"2", "controller did not acknowledge restored terminal state"
    return code


def fake_provider():
    record = {
        "provider": os.environ["AGF_PTY_PROVIDER"],
        "argv": sys.argv[2:], "cwd": os.getcwd(),
        "env": {key: os.environ.get(key) for key in ("CLAUDE_CONFIG_DIR", "ANTIGRAVITY_CLI_HOME", "HOME", "PATH")},
        "stdin_flags": fd_flags(0), "stdout_flags": fd_flags(1),
        "termios": terminal_attributes(0), "ttys": [os.isatty(0), os.isatty(1)],
    }
    with open(os.environ["AGF_PTY_CAPTURE"], "a", encoding="utf-8") as output:
        output.write(json.dumps(record) + "\n")
    print(HANDOFF.decode(), flush=True)
    fd = int(os.environ["AGF_PTY_ACK_FD"])
    readable, _, _ = select.select([fd], [], [], STEP_TIMEOUT)
    assert readable and os.read(fd, 1) == b"1", "controller did not acknowledge inspected native handoff"


def verify_theme_case(case, run, fixture):
    if case == "theme_settings":
        run.theme_selection("dark", "PTY_FIXTURE_ONLY")
        run.resize(40, 12, ["3/3"])
        run.input_frame(F1, ["Help & Settings"], "open Help before choosing appearance")
        run.setting_input(b"\t", "Search scope", "Names and paths", "open Settings")
        run.setting_input(DOWN, "History entries", "5", "select History entries")
        run.setting_input(DOWN, "Show recap", "Off", "select Show recap")
        run.setting_input(DOWN, "Appearance", "Auto", "fourth setting is reachable")
        run.theme_selection("dark", "Appearance")
        run.setting_input(b"\r", "Appearance", "Dark", "Enter cycles Auto to Dark")
        run.theme_selection("dark", "Appearance")
        assert 'appearance = "dark"' in fixture.config.read_text(encoding="utf-8")
        run.setting_input(b" ", "Appearance", "Light", "Space cycles Dark to Light immediately")
        run.theme_selection("light", "Appearance")
        assert 'appearance = "light"' in fixture.config.read_text(encoding="utf-8")
        run.resize(20, 8, ["Appearance", "Light"])
        run.footer(["Enter", "Esc"])
        run.theme_selection("light", "Appearance")
        run.setting_input(b"\r", "Appearance", "Auto", "compact appearance cycles back to Auto")
        run.theme_selection("dark", "Appearance")
        assert "appearance" not in fixture.config.read_text(encoding="utf-8")
        run.setting_input(b" ", "Appearance", "Dark", "compact Space cycles Auto to Dark")
        run.setting_input(b"\r", "Appearance", "Light", "compact Enter selects Light")
        run.theme_selection("light", "Appearance")
        run.input_frame(b"\x1b", ["3/3"], "appearance changes return to the same Browse")
        run.theme_selection("light")
        assert 'appearance = "light"' in fixture.config.read_text(encoding="utf-8")
        assert run.background_replies == 0, "appearance must not add terminal background queries"
        return {"appearance_live_cycle": True, "appearance_20x8": True,
                "appearance_config_saved": "light", "no_osc_background_probe": True}

    appearance = "light" if fixture.appearance == "auto_light" else fixture.appearance
    run.theme_selection(appearance, "PTY_FIXTURE_ONLY")
    brands = run.theme_brands(appearance)
    run.theme_snapshot("browse")
    run.input_frame(b"\r", ["Pin Session"], "open actions to pin the isolated Claude session")
    run.theme_selection(appearance, "1) Resume Session")
    run.input_frame(b"5", [">*", "3/3"], "pin without executing the provider")
    run.theme_selection(appearance, "PTY_FIXTURE_ONLY")
    run.theme_token(appearance, "*", "selection_text", selected=True)
    if appearance in BASIC_APPEARANCES:
        run.frame(["+ Settings saved"], 0, "Basic success retains its explicit plus marker")
    run.theme_snapshot("pinned_selected")
    run.marked_input(DOWN, "PTY_OTHER_01", "Down moves the visible selection marker")
    run.theme_selection(appearance, "PTY_OTHER_01")
    run.theme_token(appearance, "*", "selection_muted", selected=False)
    run.theme_snapshot("pinned_unselected")
    assert not run.screen.is_marked("PTY_FIXTURE_ONLY", 0)
    run.marked_input(UP, "PTY_FIXTURE_ONLY", "Up restores the first selection")
    run.search(b"?[]", "?[]", "1/3", "palette preserves literal search")
    run.theme_selection(appearance, "PTY_LITERAL_?[]")
    run.search(LEFT + b"x", "?[x]", "0/3", "palette preserves Left/caret editing", caret=3)
    run.search(b"\x7f" + RIGHT, "?[]", "1/3", "palette preserves Right/caret editing")
    run.input_frame(b"\x15", ["3/3"], "clear theme query")
    # Punctuation's best match can be in the duplicate full path. This query
    # has visible project-name positions, so every matched glyph must underline.
    run.search(b"PTY_LITERAL", "PTY_LITERAL", "1/3", "visible project match keeps its underline")
    run.theme_selection(appearance, "PTY_LITERAL_?[]")
    for row, line in enumerate(run.screen.completed[3].splitlines()):
        if line.lstrip().startswith(">"):
            start = line.index("PTY_LITERAL")
            assert all(cell.underscore for cell in run.screen.completed_cells[row][start:start + 11]), \
                "visible query matches lost their non-color underline"
    run.input_frame(b"\x15", ["3/3"], "clear highlighted query")
    for width, height in ((40, 12), (20, 8), (80, 12)):
        run.resize(width, height, ["3/3"])
        run.theme_selection(appearance)
        run.footer(["Enter", "F1", "Esc"])
        run.theme_token(appearance, "*", "selection_text", selected=True)
        run.theme_snapshot("browse_{}x{}".format(width, height))
    run.resize(96, 24, ["3/3", "PTY_FIXTURE_ONLY"])
    run.input_frame(b"\r", ["Resume Session"], "theme Enter opens actions without launching")
    run.theme_selection(appearance, "1) Resume Session")
    assert assert_marker_roles(run.screen, appearance)["menu_number"] >= 2
    run.theme_snapshot("actions")
    run.marked_input(b"\t", "2) New Session", "theme Tab moves action selection")
    run.theme_selection(appearance, "2) New Session")
    run.marked_input(b"\x1b[Z", "1) Resume Session", "theme Shift-Tab restores resume action")
    run.input_frame(b"\r", ["Resume mode for Claude Code"], "theme resume opens modes without launching")
    run.theme_selection(appearance, "default")
    run.marked_input(DOWN, "acceptEdits", "permission selection is recognizable without color")
    run.theme_selection(appearance, "acceptEdits")
    run.theme_snapshot("permissions")
    run.input_frame(b"\x1b", ["Resume Session"], "Escape backs out of the permission picker")
    run.input_frame(b"\x1b", ["3/3"], "Escape restores Browse without launching")
    run.input_frame(b"\x07", ["Project View"], "group headers keep a non-color selection marker")
    selected = run.theme_selection(appearance)
    if "\u25b8" in selected:
        run.input_frame(b" ", ["Project View", "\u25be"], "expand the selected group")
    run.marked_input(DOWN, "", "group child keeps a selection marker")
    selected = run.theme_selection(appearance)
    assert "\u2514\u2500" in selected or "\u251c\u2500" in selected, "Down did not select a group child"
    assert_grouped_roles(run.screen, appearance, brands)
    run.theme_snapshot("grouped_claude")
    for _ in range(6):
        run.marked_input(DOWN, "", "inspect the next grouped row's color roles")
        selected = run.theme_selection(appearance)
        if "\u25b8" in selected:
            run.input_frame(b" ", ["Project View", "\u25be"], "expand another provider's group")
            run.theme_selection(appearance)
        if "codex" in selected:
            break
    assert "codex" in selected, "group navigation did not reach the isolated Codex child"
    assert assert_grouped_roles(run.screen, appearance, brands) == {"claude", "codex"}
    run.theme_snapshot("grouped_codex")
    run.input_frame(b"\x1b", ["3/3"], "group Escape returns to Browse")
    run.input_frame(F1, ["Help & Settings"], "theme F1 opens Help")
    run.marked_input(b"\t", "Search scope", "Settings remains recognizable in every palette")
    run.theme_selection(appearance, "Search scope")
    run.theme_snapshot("settings")
    for label in ("History entries", "Show recap", "Appearance"):
        run.marked_input(DOWN, label, "inspect compact Settings without changing appearance")
        run.theme_selection(appearance, label)
    run.resize(20, 8, ["Appearance"])
    run.theme_selection(appearance, "Appearance")
    run.footer(["Enter", "Esc"])
    run.theme_snapshot("settings-20x8")
    run.resize(96, 24, ["Appearance"])
    run.input_frame(b"\x1b", ["3/3"], "Help Escape restores Browse")
    run.input_frame(b"\x04", ["DELETE MODE", "0 selected"], "bulk row has a selection marker")
    run.theme_selection(appearance)
    run.input_frame(b" ", ["1 selected", "[x]"], "checked row keeps a neutral pointer and red checkbox")
    run.theme_selection(appearance)
    run.theme_token(appearance, "[x]", "danger", selected=False)
    run.marked_input(UP, "PTY_FIXTURE_ONLY", "return to the checked row after Space advances selection")
    run.theme_selection(appearance)
    run.theme_token(appearance, "[x]", "danger", selected=True)
    run.theme_snapshot("bulk_checked")
    run.marked_input(DOWN, "", "checked unselected row keeps its red checkbox")
    run.theme_selection(appearance)
    run.theme_token(appearance, "[x]", "danger", selected=False)
    run.input_frame(b"\r", ["Delete 1 sessions?"], "theme confirms destructive scope without acting")
    run.theme_selection(appearance, "Cancel")
    run.theme_token(appearance, "Yes, delete all", "danger", selected=False)
    run.marked_input(LEFT, "Yes, delete all", "danger choice keeps a selection marker")
    run.theme_selection(appearance, "Yes, delete all")
    run.theme_token(appearance, "Yes, delete all", "danger", selected=True)
    run.theme_snapshot("delete_confirm")
    run.input_frame(b"\x1b", ["DELETE MODE", "1 selected"], "Escape cancels destructive choice")
    run.input_frame(b"\x1b", ["3/3"], "leave bulk selection")
    if appearance in BASIC_APPEARANCES:
        # Only the fixture's config is obstructed; provider stores remain read-only.
        fixture.config.unlink()
        fixture.config.mkdir()
        run.input_frame(F2, ["! Settings not saved"], "Basic save failure retains its explicit warning marker")
        run.theme_selection(appearance)
    assert run.background_replies == 0, "theme selection must not add terminal background queries"
    return {"appearance": appearance, "selection_marker": True, "whole_footer_keys": True,
            "literal_query_and_caret": True, "search_underline": True,
            "neutral_markers_pins_numbers": True, "agent_brand_colors": brands,
            "grouped_only_agent_is_branded": True, "danger_label_and_checkbox": True,
            "footer_accent": True,
            "selected_rgb_contrast_minimum": None if appearance == "no_color" or appearance in BASIC_APPEARANCES else 4.5,
            "basic_monochrome_fallback": appearance in BASIC_APPEARANCES,
            "basic_status_markers": appearance in BASIC_APPEARANCES,
            "physical_ansi_palette_contrast": "not_measured" if appearance in BASIC_APPEARANCES else "not_applicable",
            "no_color_sgr": appearance == "no_color", "no_osc_background_probe": True,
            "action_permission_group_settings_delete_navigation": True}


def verify_watch_case(case, run, fixture):
    appearance = case.removeprefix("watch_")
    assert fixture.environment(run.ack_read)["PATH"] == "4001"
    assert {path.name for path in fixture.bin.iterdir()} == {"claude", "codex", "sh"}, \
        "watch fixture must not discover real providers or pgrep"
    run.theme_selection(appearance, "PTY_LITERAL_?[]")
    brands = run.theme_brands(appearance)
    for label in ("PTY_OTHER_01", "PTY_OTHER_02"):
        run.marked_input(DOWN, label, "watch selection moves without a provider action")
        run.theme_selection(appearance, label)
    checked = set()
    for line, cells in zip(run.screen.completed[3].splitlines(), run.screen.completed_cells):
        match = re.match(r"^[ >]+\?\s+(Claude Code|Codex)\s", line)
        if match:
            checked.add(match[1])
            for x, cell in enumerate(cells):
                if cell.data.strip() and not match.start(1) <= x < match.end(1):
                    assert is_neutral(cell.fg), "watch metadata retained a brand hue: {}".format(cell)
    assert checked == {"Claude Code", "Codex"}, "watch must show both labels and unknown-status markers"
    run.theme_snapshot("watch")
    for width, height in ((40, 12), (20, 8)):
        run.resize(width, height, ["agf watch"])
        run.theme_selection(appearance)
        run.footer(["q", "Esc"])
    run.resize(96, 24, ["agf watch", "running status unknown", "PTY_OTHER_02"])
    run.theme_selection(appearance, "PTY_OTHER_02")
    assert run.background_replies == 0, "watch must not add terminal background probes"
    return {"appearance": appearance, "watch_color_roles": True,
            "watch_unknown_process_status": True, "watch_compact_resize": True,
            "agent_brand_colors": brands, "provider_executions": 0,
            "no_real_process_probe": True, "selected_rgb_contrast_minimum": 4.5}


def verify_ui_case(case, run, fixture):
    if case == "literal_query_caret":
        run.search(b"?[]", "?[]", "1/3", "question mark and brackets remain literal search input")
        run.assert_absent("Help & Settings")
        run.assert_absent("Session Detail")
        run.search(LEFT + b"x", "?[x]", "0/3", "Left edits the search caret", caret=3)
        run.search(b"\x7f" + RIGHT + b"x", "?[]x", "0/3", "Right edits the search caret")
        run.search(b"\x7f", "?[]", "1/3", "Backspace restores the literal match")
        run.input_frame(b"\x15", ["3/3"], "clear literal query")
        run.input_frame(F3, ["DETAIL_09"], "F3 wraps to the previous summary")
        run.input_frame(F4, ["PTY_FIXTURE_ONLY"], "F4 selects the next summary")
        run.search(b"DETAIL_01", "DETAIL_01", "0/3", "summaries are excluded by default")
        run.input_frame(F2, ["1/3", "All text"], "F2 opts into summary search and shows its scope")
        run.input_frame(F2, ["0/3", "Name/path"], "F2 disables summary search and shows its scope")
        return {"literal_question_brackets": True, "left_right_caret": True,
                "f3_f4_summary_cycle": True, "summary_search_opt_in": True}

    if case == "help_compact":
        run.resize(40, 12, ["3/3"])
        run.input_frame(F1, ["Help & Settings", "Keys", "Settings"], "F1 opens compact help")
        top = run.screen.completed[3]
        for key, name in ((PAGE_DOWN, "PageDown"), (PAGE_UP, "PageUp"),
                          (END, "End"), (HOME, "Home"),
                          (WHEEL_DOWN, "wheel down"), (WHEEL_UP, "wheel up")):
            run.changed_content(key, "Keys scrolls using " + name)
            run.footer(["Tab", "Esc"])
            if key in (HOME, WHEEL_UP):
                assert run.screen.completed[3] == top, "Keys Home/wheel did not return to the first page"
        run.setting_input(b"\t", "Search scope", "Names and paths", "Tab reaches Settings at 40x12")
        run.footer(["Tab", "Enter", "Esc"])
        run.setting_input(b"\r", "Search scope", "Names, paths + history", "Enter toggles search scope")
        run.setting_input(b" ", "Search scope", "Names and paths", "Space toggles search scope back")
        run.setting_input(DOWN, "History entries", "5", "Down reaches summary count")
        run.footer(["Tab", "+/-", "Esc"])
        assert not run.screen.footer_matches(["Enter"], 0), "count footer advertises a no-op Enter action"
        run.setting_input(b"+", "History entries", "6", "plus increases history entry count")
        run.setting_input(b"-", "History entries", "5", "minus decreases history entry count")
        run.setting_input(DOWN, "Show recap", "Off", "last Settings field remains reachable")
        run.setting_input(b" ", "Show recap", "On", "Space toggles recap")
        run.setting_input(b"\r", "Show recap", "Off", "Enter toggles recap back")
        run.setting_input(UP, "History entries", "5", "Up returns to the preceding Settings field")
        run.setting_input(UP, "Search scope", "Names and paths", "select scope for minimum-size editing")
        run.resize(20, 8, ["Search scope", "Names and paths"])
        run.footer(["Enter", "Esc"])
        run.setting_input(b"\r", "Search scope", "All text", "20x8 Enter enables summary search")
        run.resize(40, 12, ["Search scope", "Names, paths + history"])
        run.resize(20, 8, ["Search scope", "All text"])
        run.setting_input(b" ", "Search scope", "Names and paths", "20x8 Space disables summary search")
        run.resize(40, 12, ["Search scope", "Names and paths"])
        run.input_frame(b"\x1b[Z", ["[Keys] / Settings"], "Shift-Tab returns to Keys")
        run.input_frame(F1, ["3/3"], "F1 returns from Help")
        run.input_frame(b"\r", ["Resume Session"], "open actions before global Help")
        run.input_frame(F1, ["Help & Settings"], "global F1 opens Help from actions")
        run.input_frame(b"\x1b", ["Resume Session"], "Help Escape restores the calling mode")
        run.input_frame(b"\x1b", ["3/3"], "Escape returns actions to Browse")
        return {"help_40x12_keys_scroll": True, "settings_all_fields_reachable": True,
                "settings_toggle_count_controls": True, "settings_scope_toggle_20x8": True,
                "global_f1_return_mode": True}

    if case == "details_scroll":
        run.resize(40, 12, ["3/3"])
        run.input_frame(b"\x0c", ["Session Detail", "PTY_LITERAL_?[]"], "Ctrl-L opens long details")
        run.assert_absent("DETAILS_END_09")
        top = run.screen.completed[3]
        run.changed_content(PAGE_DOWN, "Details PageDown scrolls contents")
        run.changed_content(PAGE_UP, "Details PageUp returns toward the top")
        assert run.screen.completed[3] == top, "Details PageUp failed to restore the first page"
        run.input_frame(END, ["DETAILS_END_09"], "End exposes the final wrapped summary tail")
        run.footer(["Enter", "Esc"])
        run.input_frame(HOME, ["PTY_LITERAL_?[]"], "Home restores details metadata")
        assert run.screen.completed[3] == top, "Details Home failed to restore the first page"
        run.changed_content(WHEEL_DOWN, "Details mouse wheel scrolls contents")
        run.changed_content(WHEEL_UP, "Details mouse wheel returns to top")
        assert run.screen.completed[3] == top, "Details wheel scrolled sessions instead of contents"
        run.input_frame(DOWN, ["PTY_OTHER_01"], "Details Down still cycles sessions")
        run.input_frame(UP, ["PTY_LITERAL_?[]"], "Details Up still cycles sessions")
        run.input_frame(F1, ["Help & Settings"], "global Help opens from Details")
        run.input_frame(F1, ["Session Detail", "PTY_LITERAL_?[]"], "F1 restores Details")
        run.input_frame(b"\r", ["Resume Session"], "Details Enter still opens actions")
        run.input_frame(b"\x1b", ["3/3"], "leave actions without launching")
        run.input_frame(b"\x0c", ["Session Detail"], "reopen details for Left-back")
        run.input_frame(LEFT, ["3/3"], "Details Left returns to Browse")
        run.input_frame(b"\x0c", ["Session Detail"], "reopen details for Escape-back")
        run.input_frame(b"\x1b", ["3/3"], "Details Escape returns to Browse")
        return {"details_40x12_wrapped_tail": True, "details_key_wheel_scroll": True,
                "details_up_down_session_cycle": True, "details_enter_left_escape": True}

    if case == "footer_widths":
        for columns in (20, 39, 40, 80, 120):
            run.resize(columns, 12, [])
            run.footer(["Enter", "F1", "Esc"])
            footer = run.screen.completed[3].splitlines()[-1]
            assert "?" not in footer and "[ or ]" not in footer, "obsolete Browse shortcut labels"
            assert "\u2026" not in footer and not footer.rstrip().endswith("..."), "footer keys were truncated"
        run.resize(20, 8, ["3/3"])
        run.footer(["Enter", "F1", "Esc"])
        run.assert_absent("Resize terminal")
        run.resize(20, 7, ["Resize terminal"], rule=False)
        run.footer(["Esc"])
        run.input_frame(b"\r", ["Resize terminal"], "undersized viewport cannot activate hidden actions")
        run.assert_absent("Resume Session")
        run.resize(120, 12, ["3/3"])
        run.input_frame(F1, ["Help & Settings"], "inspect wide Help binding labels")
        labels = set()
        for _ in range(8):
            text = run.screen.completed[3]
            labels.update(key for key in ("F1", "F2", "F3", "F4", "Ctrl+L", "Ctrl+U", "Ctrl+G", "Ctrl+S", "Ctrl+D")
                          if key in text)
            before = text
            run.input_frame(PAGE_DOWN, ["Help & Settings"], "page through displayed key labels")
            if run.screen.completed[3] == before:
                break
        assert labels == {"F1", "F2", "F3", "F4", "Ctrl+L", "Ctrl+U", "Ctrl+G", "Ctrl+S", "Ctrl+D"}, labels
        run.input_frame(b"\x1b", ["3/3"], "Help Escape returns to Browse")
        return {"footer_whole_essential_keys": True, "widths": [20, 39, 40, 80, 120],
                "help_binding_labels": True, "minimum_20x8_supported": True,
                "undersized_actions_disabled": True}

    if case == "query_selection_reset":
        for i in range(1, 13):
            run.selected_input(DOWN, "PTY_ROW_{:02}".format(i), "navigate away from the first match")
        run.selected_input(b"PTY_ROW_", "PTY_ROW_01", "query edit selects the first matching row")
        run.selected_input(DOWN, "PTY_ROW_02", "navigate inside filtered results")
        run.selected_input(b"0", "PTY_ROW_01", "refined query resets selection even when old row still matches")
        run.selected_input(DOWN, "PTY_ROW_02", "select second filtered result again")
        run.selected_input(b"\t", "PTY_ROW_01", "provider filter change resets selection")
        run.selected_input(DOWN, "PTY_ROW_02", "move within selected provider")
        run.selected_input(b"\x1b[Z", "PTY_ROW_01", "reverse provider filter change resets selection")
        run.selected_input(b"\x15", "PTY_FIXTURE_ONLY", "clearing the query selects the first result")
        run.input_frame(b"\x1b[200~UNIQUE_NEEDLE\x1b[201~\r", ["Resume Session", "PTY_ROW_19"],
                        "Paste before Enter opens actions for the newly filtered session")
        assert not fixture.capture.exists(), "paste plus Enter skipped the action menu"
        run.input_frame(b"\x1b", ["PTY_ROW_19", "1/20"], "return to the pasted query")
        run.input_frame(b"\r\x1b[200~INACTIVE_PASTE\x1b[201~", ["Resume Session", "PTY_ROW_19"],
                        "Paste after Enter cannot mutate the inactive search")
        run.input_frame(b"\x1b", ["PTY_ROW_19", "1/20"], "inactive paste did not leak into Browse")
        run.assert_absent("INACTIVE_PASTE")
        return {"query_first_match_reset": True, "provider_first_match_reset": True,
                "clear_first_match_reset": True, "paste_enter_event_order": True}

    if case in FAILURE_CASES:
        run.frame(["Refresh failed: Antigravity"], 0, "provider scan error is visible in Browse")
        if fixture.empty:
            run.assert_absent("No saved sessions")
            run.assert_absent("Cached results")
        else:
            assert "PTY_FIXTURE_ONLY" in run.screen.completed[3], "healthy provider sessions disappeared"
        run.search(b"NO_MATCH_FOR_BROKEN_STORE", "NO_MATCH_FOR_BROKEN_STORE",
                   "0/{}".format(fixture.session_count), "filter does not hide the scan failure")
        assert "Sessions unavailable" in run.screen.completed[3]
        assert "Refresh failed: Antigravity" in run.screen.completed[3]
        run.assert_absent("No matches")
        run.footer(["Enter", "F1", "Esc"])
        return {"failed_provider_visible": True, "empty_provider_failure": fixture.empty,
                "failure_not_silent_empty_result": True}

    raise AssertionError("unknown UI case " + case)


def run_case(case, executable, cells_path=None):
    binary_hash = hashlib.sha256()
    with executable.open("rb") as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            binary_hash.update(chunk)
    with Fixture(antigravity=case.startswith("antigravity_"),
                 all_providers=case in ("mouse_and_long_menu", "query_selection_reset"),
                 ui_polish=case in UI_CASES | THEME_CASES | WATCH_CASES, failed_provider=case in FAILURE_CASES,
                 empty=case == "scan_failure_empty", unique_query=case == "query_selection_reset",
                 appearance=case.split("_", 1)[1] if case in THEME_CASES | WATCH_CASES else None,
                 codex_brand=case in (THEME_CASES - {"theme_settings"}) | WATCH_CASES) as fixture:
        with PtyRun(executable, fixture, ("--version",), prime_stdout=False) as control:
            baseline = control.stdio_control()
        report = run_tui_case(case, executable, fixture, binary_hash.hexdigest(), baseline,
                              capture_cells=cells_path is not None)
        if case == "theme_settings":
            with PtyRun(executable, fixture) as reopened:
                reopened.initial()
                reopened.theme_selection("light", "PTY_FIXTURE_ONLY")
                reopened.send(b"\x1b")
                reopened.finish()
                assert not fixture.capture.exists(), "appearance reload executed a provider"
            report["checks"]["appearance_reloaded"] = True
        frames = report.pop("theme_frames")
        if cells_path is not None:
            assert frames, "cell export requires a theme case with captured frames"
            cells_path.write_text(json.dumps({**report, "frames": frames,
                "source": "completed real PTY frames; isolated provider stores; not a physical terminal screenshot"}),
                encoding="utf-8")
        return report


def run_tui_case(case, executable, fixture, binary_hash, baseline, capture_cells=False):
    arguments = ("watch", "--interval", "3600") if case in WATCH_CASES else ()
    with PtyRun(executable, fixture, arguments) as run:
        run.capture_cells = capture_cells
        run.theme_frames = {}
        assert run.original_termios == baseline["termios"], "fresh PTYs disagree on initial termios"
        assert run.original_flags == baseline["after_flags"], "primed TUI baseline differs from same-binary stdio control"
        run.initial(["agf watch", "running status unknown", "PTY_LITERAL_?[]", "PTY_OTHER_01", "PTY_OTHER_02"]
                    if case in WATCH_CASES else None)
        checks = {"initial_render": True, "real_pty": True}
        if case == "render_resize_input_quit":
            run.input_frame(b"\t3001", ["Claude Code (1)", "3001", "1/1"],
                            "Tab plus typing stays in provider search")
            run.input_frame(b"\x15", ["1/1"], "Ctrl-U clears search")
            run.resize(40, 12, ["Claude Code (1)", "1/1"])
            after = run.mark_frame()
            run.send(TYPED.encode("utf-8"))
            run.frame([TYPED, "0/1"], after, "UTF-8 search input")
            after = run.mark_frame()
            run.send(b"\x1b[200~" + PASTED.encode("utf-8") + b"\x1b[201~")
            run.frame([PASTED], after, "bracketed paste search input")
            run.resize(110, 28, [TYPED + PASTED, "No matches", "0/1"])
            run.send(b"\x1b")
            run.wait(lambda: ALT_OFF in run.output, "single Escape quit without rescue input")
            run.finish()
            assert not fixture.capture.exists(), "quit unexpectedly executed a provider"
            checks.update({"resize_small_large": True, "utf8_search": True, "bracketed_paste": True,
                           "single_escape_quit": True, "provider_executions": 0})
        elif case in ("resume_handoff", "antigravity_handoff", "antigravity_plan_handoff"):
            antigravity = fixture.antigravity
            plan = case == "antigravity_plan_handoff"
            mode_label = "plan (read-only)" if plan else "accept-edits" if antigravity else "acceptEdits"
            if antigravity:
                run.input_frame(b"\x04", ["DELETE MODE", "0 selected"], "Antigravity bulk delete mode")
                run.input_frame(b" \r", ["DELETE MODE", "0 selected"], "native-managed store cannot be selected for deletion")
                run.assert_absent("[x]")
                run.input_frame(b"\x1b", ["All (1)", "PTY_FIXTURE_ONLY"], "Escape cancels bulk delete")
            after = run.mark_frame()
            run.send(b"\t\r")
            run.frame(["Resume Session", "New Session"], after, "batched Tab and Enter open only the action menu")
            assert not fixture.capture.exists(), "batched navigation skipped confirmation"
            if antigravity:
                run.assert_absent("Delete Session")
            after = run.mark_frame()
            run.send(b"\r")
            run.frame(["Resume mode for", mode_label], after, "Enter opens resume mode picker")
            run.selected_input(b"\x1b[B", "accept-edits" if antigravity else "acceptEdits", "Down selects edit mode")
            if plan:
                run.selected_input(b"\x1b[B", mode_label, "Down selects plan mode")
            run.send(b"\r")
            run.wait(lambda: HANDOFF in run.output, "Enter performs native handoff")
            boundary = bytes(run.output).index(HANDOFF)
            check_cleanup(bytes(run.output[:boundary]))
            records = fixture.capture.read_text(encoding="utf-8").splitlines()
            assert len(records) == 1, "only one synthetic CLI execution is allowed"
            record = json.loads(records[0])
            expected_args = (["--conversation", CONVERSATION_ID, "--mode", "plan" if plan else "accept-edits"]
                             if antigravity else ["--resume", SESSION_ID, "--permission-mode", "acceptEdits"])
            assert record["provider"] == ("agy" if antigravity else "claude"), record
            assert record["argv"] == expected_args, record
            assert record["cwd"] == str(fixture.project), record
            assert record["env"] == {"CLAUDE_CONFIG_DIR": "5001" if antigravity else str(fixture.storage),
                                     "ANTIGRAVITY_CLI_HOME": None,
                                     "HOME": str(fixture.home), "PATH": "4001"}, record
            assert record["ttys"] == [True, True], record
            assert record["termios"] == run.original_termios, "raw mode was not restored before native execution"
            assert [record["stdin_flags"], record["stdout_flags"]] == run.original_flags, "fd flags leaked into native execution"
            run.assert_restored()
            assert os.write(run.ack_write, b"1") == 1
            run.finish()
            checks.update({"keyboard_action_mode_resume": True, "literal_argv_cwd_storage_env": True,
                           "batched_tab_enter_confirmation": True,
                           "cleanup_before_handoff": True, "provider_executions": 1})
            if antigravity:
                checks.update({"antigravity_exact_conversation": True, "delete_disabled_ui": True,
                               "antigravity_storage": "isolated_home_default"})
        elif case == "mouse_and_long_menu":
            assert modes_at(bytes(run.output)).get(1006), "SGR mouse protocol not enabled"
            for i in range(1, fixture.session_count):
                run.selected_input(b"\x1b[<65;5;5M", "PTY_ROW_{:02}".format(i), "mouse wheel selects next session")
            run.resize(80, 12, ["PTY_ROW_19", "20/20"])
            run.selected_input(b"\x1b[<64;5;5M", "PTY_ROW_18", "mouse wheel up after resize")
            run.input_frame(b"\x1b[<0;2;12M\x1b[<0;2;12m", ["All (20)", "20/20"], "footer click is not a session")
            run.assert_absent("Resume Session")
            top_row = run.screen.completed[3].splitlines()[3]
            target = re.search(r"PTY_ROW_\d+", top_row).group()
            run.input_frame(b"\x1b[<0;2;4M\x1b[<0;2;4m", ["Resume Session", target], "click resolves current resized row")
            run.selected_input(b"\t", "2) New Session", "Tab selects new session action")
            run.input_frame(b"\r", ["New session in", "1) Claude Code"], "open fifteen-provider menu")
            run.input_frame(b"1", ["Select mode for Claude Code"], "agent digit opens modes without launching")
            assert not fixture.capture.exists(), "agent digit skipped the mode picker"
            run.input_frame(b"\x1b", ["New session in", "1) Claude Code"], "return from digit-selected modes")
            for index, label in enumerate(("Codex", "Grok Build", "Kimi Code", "Qwen Code", "OpenCode", "pi", "Oh My Pi",
                                           "Kiro", "Cursor CLI", "Gemini", "Hermes", "Yolop", "Prime Agent", "Antigravity"), 2):
                run.selected_input(b"\t", "{}) {}".format(index, label), "navigate long provider menu")
            run.resize(110, 28, ["15) Antigravity", "New session in"])
            run.selected_input(b"\t", "1) Claude Code", "provider menu wraps forward")
            run.selected_input(b"\x1b[Z", "15) Antigravity", "provider menu wraps backward")
            run.input_frame(b"\r", ["Select mode for Antigravity", "accept-edits"], "new Antigravity mode picker")
            run.input_frame(b"\x1b[200~inactive\x1b[201~", ["Select mode for Antigravity"], "inactive search ignores paste")
            for anchors in (["New session in"], ["Resume Session"], ["All (20)", "20/20"]):
                run.input_frame(b"\x1b", anchors, "Escape restores previous mode")
            run.input_frame(b"\tPTY_ROW_", ["Claude Code (20)", "PTY_ROW_", "19/20"], "Tab plus typing after returning to browse")
            run.input_frame(b"\x15", ["Claude Code (20)", "20/20"], "clear filter before cancellation test")
            run.input_frame(b"\x04", ["DELETE MODE", "0 selected"], "enter bulk selection")
            run.input_frame(b" ", ["1 selected"], "select one synthetic session")
            run.input_frame(b"\r", ["Delete 1 sessions?"], "open delete confirmation")
            run.selected_input(b"\x1b[D", "Yes, delete all", "select destructive choice without confirming")
            # CSI-u disambiguates Escape from Alt+Enter within one input batch.
            try:
                run.input_frame(b"\x1b[27u\r", ["DELETE MODE", "1 selected"], "Escape wins over a simultaneous confirmation")
            except AssertionError as error:
                raise AssertionError(f"{error}; provider_files_unchanged={fixture.provider_files() == fixture.original_files}") from error
            assert fixture.provider_files() == fixture.original_files, "cancelled deletion changed provider files"
            run.input_frame(b"\x1b", ["Claude Code (20)", "20/20"], "leave bulk mode after cancellation")
            run.send(b"\x03")
            run.finish()
            assert not fixture.capture.exists(), "menu navigation executed a provider"
            checks.update({"sgr_mouse_scroll_click": True, "resize_selection_identity": True,
                           "fifteen_provider_menu": True, "new_antigravity_modes": True,
                           "inactive_input_and_focus_return": True, "ctrl_c_quit": True,
                           "escape_enter_cancels_destructive_action": True,
                           "provider_executions": 0})
        elif case == "ctrl_c_quit":
            run.input_frame(F1, ["Settings"], "open help before Ctrl-C")
            run.send(b"\x03")
            run.finish()
            assert not fixture.capture.exists()
            checks.update({"ctrl_c_quit": True, "provider_executions": 0})
        elif case in WATCH_CASES:
            checks.update(verify_watch_case(case, run, fixture))
            run.send(b"q")
            run.finish()
            assert not fixture.capture.exists(), "watch unexpectedly executed a provider"

        elif case in UI_CASES | FAILURE_CASES | THEME_CASES or case == "query_selection_reset":
            checks.update(verify_theme_case(case, run, fixture) if case in THEME_CASES
                          else verify_ui_case(case, run, fixture))
            run.send(b"\x1b")
            run.finish()
            assert not fixture.capture.exists(), "UI inspection unexpectedly executed a provider"
            checks["provider_executions"] = 0
        else:
            raise AssertionError("unknown case " + case)
        checks.update({"termios_restored": True, "stdin_stdout_flags_restored": True,
                       "alternate_screen_cleanup": True, "fixture_history_unchanged": True})
        return {"case": case, "passed": True, "platform": sys.platform, "checks": checks,
                "theme_frames": run.theme_frames,
                "agf_binary_sha256": binary_hash, "screen_emulator": "pyte",
                "stdio_control": {"before_flags": baseline["before_flags"], "after_flags": baseline["after_flags"],
                                  "termios_unchanged": True, "version": baseline["version"]},
                "protocol_replies": run.protocol_replies, "real_provider_sessions": "not_exercised",
                "os_ime_composition": "not_exercised", "windows_console": "not_exercised"}


if __name__ == "__main__":
    def deadline_expired(*_):
        raise TimeoutError("overall PTY controller deadline exceeded")

    signal.signal(signal.SIGALRM, deadline_expired)
    signal.alarm(45)
    if sys.argv[1] == "fake":
        fake_provider()
    elif sys.argv[1] == "supervise":
        sys.exit(supervise(sys.argv[2], sys.argv[3:]))
    else:
        print(json.dumps(run_case(sys.argv[1], Path(sys.argv[2]).resolve(),
                                  Path(sys.argv[3]) if len(sys.argv) > 3 else None)), flush=True)
