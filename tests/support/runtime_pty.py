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
SESSION_ID = "pty-id ' ; printf UNEXPECTED_EXECUTION ; $HOME"
TYPED = "\ud55c\uae00"
PASTED = "\ubd99\uc5ec\ub123\uae30"


class FrameScreen(pyte.Screen):
    """Let pyte interpret VT bytes; publish only completed synchronized frames."""

    def __init__(self, columns, lines):
        self.frame_serial = 0
        self.geometry_epoch = 0
        self.completed = None
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

    def resize(self, lines=None, columns=None):
        previous = (self.columns, self.lines)
        super().resize(lines=lines, columns=columns)
        if (self.columns, self.lines) != previous:
            self.geometry_epoch += 1
            self.completed = None
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


class Fixture:
    def __enter__(self):
        # Numeric path components cannot accidentally match the Unicode queries.
        self.root = Path(tempfile.gettempdir()).resolve() / str(uuid.uuid4().int)
        self.root.mkdir(mode=0o700)
        self.home = self.root / "1001"
        self.launch = self.root / "2001"
        self.project = self.root / "3001"
        self.bin = self.launch / "4001"
        self.storage = self.launch / "5001"
        self.capture = self.root / "7001"
        self.exit_capture = self.root / "7002"
        for directory in (self.home, self.launch, self.project, self.bin, self.storage / "projects" / "6001"):
            directory.mkdir(parents=True, exist_ok=True)
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
        self.original_files = {path: path.read_bytes() for path in (self.history, self.transcript)}
        helper = str(Path(__file__).resolve())
        stub = "#!/bin/sh\nexec {} -I -X utf8 {} fake \"$@\"\n".format(
            shlex.quote(sys.executable), shlex.quote(helper)
        )
        (self.bin / "claude").write_text(stub, encoding="utf-8")
        (self.bin / "claude").chmod(0o700)
        # Only the explicit system shell and our fake provider are on PATH.
        (self.bin / "sh").symlink_to("/bin/sh")
        return self

    def environment(self, ack_fd):
        return {
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

    def assert_history_unchanged(self):
        for path, original in self.original_files.items():
            assert path.read_bytes() == original, "TUI mutated fixture history/transcript"

    def __exit__(self, *_):
        shutil.rmtree(self.root)


class PtyRun:
    def __init__(self, executable, fixture, arguments=(), prime_stdout=True):
        self.fixture = fixture
        self.output = bytearray()
        self.phase = "startup"
        self.protocol_replies = 0
        self.answered = set()
        self.screen = FrameScreen(96, 24)
        self.stream = pyte.ByteStream(self.screen)
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
            print("PTY failure phase: {} (pid {})\nVisible tail: {}\nControl tail: {!r}".format(
                self.phase, self.process.pid, visible_bytes(bytes(self.output))[-1500:], bytes(self.output[-500:])), file=sys.stderr)
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
        replies = [
            (rb"\x1b\[6n", b"\x1b[1;1R"),
            (rb"\x1b\[(?:0)?c", b"\x1b[?1;2c"),
            (rb"\x1b\[>c", b"\x1b[>0;0;0c"),
            (rb"\x1b\[\?u", b"\x1b[?0u"),
            (rb"\x1b\[\?2026\$p", b"\x1b[?2026;2$y"),
            (rb"\x1b\]11;\?(?:\x07|\x1b\\)", b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
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

    def initial(self):
        self.frame(["All (1)", "1/1", "PTY_FIXTURE_ONLY"], 0, "initial populated render")
        assert ALT_ON in self.output and PASTE_ON in self.output
        assert not terminal_attributes(self.slave)[3] & (termios.ICANON | termios.ECHO), "TUI did not acquire raw input"
        assert not self.fixture.capture.exists(), "provider ran before a resume action"
        assert [fd_flags(self.input_fd), fd_flags(self.output_fd)] == self.original_flags, "active TUI changed stdin/stdout flags"

    def resize(self, columns, rows, anchors):
        after = self.mark_frame()
        self.set_size(columns, rows)
        os.killpg(self.process.pid, signal.SIGWINCH)
        segment = self.frame(anchors, after, "SIGWINCH {}x{} render".format(columns, rows))
        cleared = segment.rfind(b"\x1b[2J")
        assert cleared >= 0, "resize did not redraw the terminal"
        positions = [(int(row), int(col)) for row, col in re.findall(rb"\x1b\[(\d+);(\d+)H", segment[cleared:])]
        assert positions and max(col for _, col in positions) >= columns - 10, "new terminal width was not rendered"
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
        "argv": sys.argv[2:], "cwd": os.getcwd(),
        "env": {key: os.environ.get(key) for key in ("CLAUDE_CONFIG_DIR", "HOME", "PATH")},
        "stdin_flags": fd_flags(0), "stdout_flags": fd_flags(1),
        "termios": terminal_attributes(0), "ttys": [os.isatty(0), os.isatty(1)],
    }
    with open(os.environ["AGF_PTY_CAPTURE"], "a", encoding="utf-8") as output:
        output.write(json.dumps(record) + "\n")
    print(HANDOFF.decode(), flush=True)
    fd = int(os.environ["AGF_PTY_ACK_FD"])
    readable, _, _ = select.select([fd], [], [], STEP_TIMEOUT)
    assert readable and os.read(fd, 1) == b"1", "controller did not acknowledge inspected native handoff"


def run_case(case, executable):
    binary_hash = hashlib.sha256()
    with executable.open("rb") as binary:
        for chunk in iter(lambda: binary.read(1024 * 1024), b""):
            binary_hash.update(chunk)
    with Fixture() as fixture:
        with PtyRun(executable, fixture, ("--version",), prime_stdout=False) as control:
            baseline = control.stdio_control()
        return run_tui_case(case, executable, fixture, binary_hash.hexdigest(), baseline)


def run_tui_case(case, executable, fixture, binary_hash, baseline):
    with PtyRun(executable, fixture) as run:
        assert run.original_termios == baseline["termios"], "fresh PTYs disagree on initial termios"
        assert run.original_flags == baseline["after_flags"], "primed TUI baseline differs from same-binary stdio control"
        run.initial()
        checks = {"initial_render": True, "real_pty": True}
        if case == "render_resize_input_quit":
            run.resize(40, 12, ["All (1)", "1/1"])
            after = run.mark_frame()
            run.send(TYPED.encode("utf-8"))
            run.frame([TYPED, "0/1"], after, "UTF-8 search input")
            after = run.mark_frame()
            run.send(b"\x1b[200~" + PASTED.encode("utf-8") + b"\x1b[201~")
            run.frame([PASTED], after, "bracketed paste search input")
            run.resize(110, 28, [TYPED + PASTED, "No sessions found", "0/1"])
            run.send(b"\x1b")
            run.wait(lambda: ALT_OFF in run.output, "single Escape quit without rescue input")
            run.finish()
            assert not fixture.capture.exists(), "quit unexpectedly executed a provider"
            checks.update({"resize_small_large": True, "utf8_search": True, "bracketed_paste": True,
                           "single_escape_quit": True, "provider_executions": 0})
        elif case == "resume_handoff":
            after = run.mark_frame()
            run.send(b"\r")
            run.frame(["Resume Session", "New Session"], after, "Enter opens action menu")
            after = run.mark_frame()
            run.send(b"\r")
            run.frame(["Resume mode for", "acceptEdits"], after, "Enter opens resume mode picker")
            after = run.mark_frame()
            run.send(b"\x1b[B")
            run.frame(["acceptEdits"], after, "Down selects acceptEdits")
            run.send(b"\r")
            run.wait(lambda: HANDOFF in run.output, "Enter performs native handoff")
            boundary = bytes(run.output).index(HANDOFF)
            check_cleanup(bytes(run.output[:boundary]))
            records = fixture.capture.read_text(encoding="utf-8").splitlines()
            assert len(records) == 1, "only one synthetic CLI execution is allowed"
            record = json.loads(records[0])
            assert record["argv"] == ["--resume", SESSION_ID, "--permission-mode", "acceptEdits"], record
            assert record["cwd"] == str(fixture.project), record
            assert record["env"] == {"CLAUDE_CONFIG_DIR": str(fixture.storage), "HOME": str(fixture.home), "PATH": "4001"}, record
            assert record["ttys"] == [True, True], record
            assert record["termios"] == run.original_termios, "raw mode was not restored before native execution"
            assert [record["stdin_flags"], record["stdout_flags"]] == run.original_flags, "fd flags leaked into native execution"
            run.assert_restored()
            assert os.write(run.ack_write, b"1") == 1
            run.finish()
            checks.update({"keyboard_action_mode_resume": True, "literal_argv_cwd_storage_env": True,
                           "cleanup_before_handoff": True, "provider_executions": 1})
        else:
            raise AssertionError("unknown case " + case)
        checks.update({"termios_restored": True, "stdin_stdout_flags_restored": True,
                       "alternate_screen_cleanup": True, "fixture_history_unchanged": True})
        return {"case": case, "passed": True, "platform": sys.platform, "checks": checks,
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
        print(json.dumps(run_case(sys.argv[1], Path(sys.argv[2]).resolve())), flush=True)
