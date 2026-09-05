"""Deterministic terminal replay regressions for the real-PTY screen matcher."""

import importlib.metadata
import importlib.util
from pathlib import Path
import unittest

import pyte


spec = importlib.util.spec_from_file_location(
    "agf_runtime_pty", Path(__file__).with_name("runtime_pty.py")
)
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)

BEGIN = b"\x1b[?2026h"
END = runtime.FRAME_END
ANCHORS = ["All (1)", "1/1", "PTY_FIXTURE_ONLY"]
EMPTY = BEGIN + b"\x1b[2;3HAll (0)" + END
POPULATED_DIFF = (BEGIN + b"\x1b[2;8H1\x1b[4;3HPTY_FIXTURE_ONLY"
                  b"\x1b[5;3H1/1" + END)


class ScreenReplayTests(unittest.TestCase):
    def test_pinned_terminal_emulator_versions(self):
        self.assertEqual(importlib.metadata.version("pyte"), "0.8.2")
        self.assertEqual(importlib.metadata.version("wcwidth"), "0.2.13")

    def make_run(self):
        run = object.__new__(runtime.PtyRun)
        run.output = bytearray()
        run.screen = runtime.FrameScreen(96, 24)
        run.stream = pyte.ByteStream(run.screen)
        run.wait = lambda predicate, phase: self.assertTrue(predicate(), phase)
        return run

    @staticmethod
    def feed(run, data):
        run.output.extend(data)
        run.stream.feed(data)

    def test_all_count_single_cell_diff_matches_current_screen(self):
        run = self.make_run()
        self.feed(run, EMPTY)
        self.feed(run, POPULATED_DIFF)
        self.assertNotIn("All (1)", runtime.visible_bytes(bytes(run.output)))
        run.frame(ANCHORS, 0, "initial populated render after scan race")

    def test_split_csi_and_sync_end_do_not_commit_partial_frames(self):
        run = self.make_run()
        self.feed(run, EMPTY)
        after = run.mark_frame()
        self.feed(run, POPULATED_DIFF[:-len(END)])
        self.assertIn("All (1)", "\n".join(run.screen.display))
        self.assertFalse(run.screen.matches(ANCHORS, after[1]))
        self.feed(run, END[:-1])
        self.assertFalse(run.screen.matches(ANCHORS, after[1]))
        self.feed(run, END[-1:])
        run.frame(ANCHORS, after, "completed split sync boundary")

    def test_frame_started_before_input_is_not_fresh_acknowledgement(self):
        run = self.make_run()
        self.feed(run, EMPTY)
        self.feed(run, POPULATED_DIFF[:-len(END)])
        after = run.mark_frame()
        self.feed(run, END)
        self.assertFalse(run.screen.matches(ANCHORS, after[1]))
        self.feed(run, BEGIN + END)
        run.frame(ANCHORS, after, "new frame after input mark")

    def test_anchors_must_coexist_not_accumulate_across_frames(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[2;3HAll (1)" + END)
        self.feed(run, BEGIN + b"\x1b[2J\x1b[4;3HPTY_FIXTURE_ONLY\x1b[5;3H1/1" + END)
        stripped = runtime.visible_bytes(bytes(run.output))
        self.assertTrue(all(anchor in stripped for anchor in ANCHORS))
        self.assertFalse(run.screen.matches(ANCHORS, 0))

    def test_resize_requires_completed_redraw_at_current_geometry(self):
        run = self.make_run()
        self.feed(run, EMPTY + POPULATED_DIFF)
        self.assertTrue(run.screen.matches(ANCHORS, 0))
        after = run.mark_frame()
        run.screen.resize(lines=12, columns=40)
        self.assertFalse(run.screen.matches(ANCHORS, 0))
        self.feed(run, BEGIN + END)
        self.assertFalse(run.screen.matches(ANCHORS, after[1]))
        self.feed(run, b"\x1b[2J" + EMPTY + POPULATED_DIFF[:-len(END)])
        self.assertFalse(run.screen.matches(ANCHORS, after[1]))
        self.feed(run, END)
        run.frame(ANCHORS, after, "completed 40x12 redraw")
        self.assertEqual(run.screen.completed[2], (40, 12))
        self.assertEqual(len(run.screen.display), 12)
        self.assertTrue(all(len(line) == 40 for line in run.screen.display))

    def test_resize_during_frame_invalidates_that_frame(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[2;3HAll (1)")
        run.screen.resize(lines=12, columns=40)
        self.feed(run, b"\x1b[2J\x1b[2;3HAll (1)" + END)
        self.assertFalse(run.screen.matches(["All (1)"], 0))
        self.feed(run, BEGIN + END)
        self.assertTrue(run.screen.matches(["All (1)"], 0))

    def test_bytewise_replay_preserves_utf8_width_and_incremental_updates(self):
        run = self.make_run()
        text = runtime.TYPED + runtime.PASTED
        data = EMPTY + POPULATED_DIFF + BEGIN + b"\x1b[1;1H" + text.encode("utf-8") + END
        for byte in data:
            self.feed(run, bytes([byte]))
        run.frame(ANCHORS + [text], 0, "bytewise UTF-8 and CSI replay")
        self.assertEqual(run.screen.cursor.x, 2 * len(text))


if __name__ == "__main__":
    unittest.main(verbosity=2)
