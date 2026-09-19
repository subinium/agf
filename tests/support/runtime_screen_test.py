"""Deterministic terminal replay regressions for the real-PTY screen matcher."""

import importlib.metadata
import importlib.util
from pathlib import Path
import unittest
from unittest import mock

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
        run.stream = runtime.SltByteStream(run.screen)
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

    def test_real_resize_wait_rejects_a_queued_old_width_redraw(self):
        run = self.make_run()
        run.screen.resize(lines=12, columns=20)
        rule = "\u2500".encode("utf-8")
        old = BEGIN + b"\x1b[2J\x1b[3;1H" + rule * 20 + END
        new = BEGIN + b"\x1b[2J\x1b[3;1H" + rule * 39 + END
        self.feed(run, old)
        run.process = mock.Mock(pid=123)
        run.set_size = lambda columns, rows: run.screen.resize(lines=rows, columns=columns)

        def await_width(predicate, phase):
            self.feed(run, old)
            self.assertFalse(predicate(), "queued old-width redraw must not acknowledge resize")
            self.feed(run, new)
            self.assertTrue(predicate(), phase)

        run.wait = await_width
        with mock.patch.object(runtime.os, "killpg") as notify:
            run.resize(39, 12, [])
        notify.assert_called_once_with(123, runtime.signal.SIGWINCH)
        self.assertEqual(run.screen.completed[2], (39, 12))

    def test_shrink_wait_rejects_old_redraw_wrapped_to_current_width(self):
        run = self.make_run()
        run.screen.resize(lines=12, columns=40)
        rule = "\u2500".encode("utf-8")
        old = (BEGIN + b"\x1b[2J\x1b[3;1H" + rule * 40
               + b"\x1b[6;1H3/3\x1b[12;1HEnter F1 Esc" + END)
        new = (BEGIN + b"\x1b[2J\x1b[3;1H" + rule * 20
               + b"\x1b[6;1H3/3\x1b[8;1HEnter F1 Esc" + END)
        self.feed(run, old)
        run.process = mock.Mock(pid=123)
        run.set_size = lambda columns, rows: run.screen.resize(lines=rows, columns=columns)

        def await_geometry(predicate, phase):
            self.feed(run, old)
            self.assertIn("\u2500" * 20, run.screen.completed[3].splitlines())
            self.assertTrue(run.screen.matches(["3/3"], 0))
            self.assertFalse(predicate(), "wrapped old output must not acknowledge smaller geometry")
            self.feed(run, new)
            self.assertTrue(predicate(), phase)

        run.wait = await_geometry
        with mock.patch.object(runtime.os, "killpg") as notify:
            run.resize(20, 8, ["3/3"])
        notify.assert_called_once_with(123, runtime.signal.SIGWINCH)
        self.assertEqual(run.screen.completed[2], (20, 8))

    def test_bytewise_replay_preserves_utf8_width_and_incremental_updates(self):
        run = self.make_run()
        text = runtime.TYPED + runtime.PASTED
        data = EMPTY + POPULATED_DIFF + BEGIN + b"\x1b[1;1H" + text.encode("utf-8") + END
        for byte in data:
            self.feed(run, bytes([byte]))
        run.frame(ANCHORS + [text], 0, "bytewise UTF-8 and CSI replay")
        self.assertEqual(run.screen.cursor.x, 2 * len(text))

    def test_selection_requires_highlight_not_only_visible_label(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[1;1H15) Antigravity" + END)
        self.assertTrue(run.screen.matches(["15) Antigravity"], 0))
        self.assertFalse(run.screen.is_selected("15) Antigravity", 0))
        after = run.mark_frame()
        self.feed(run, BEGIN + b"\x1b[1;1H\x1b[48;2;59;59;59m15) Antigravity\x1b[0m" + END)
        self.assertTrue(run.screen.is_selected("15) Antigravity", after[1]))
        after = run.mark_frame()
        self.feed(run, BEGIN + b"\x1b[1;1H\x1b[48;5;237m15) Antigravity\x1b[0m" + END)
        self.assertTrue(run.screen.is_selected("15) Antigravity", after[1]))

    def test_light_selection_accepts_only_exact_truecolor_and_slt_quantization(self):
        run = self.make_run()
        for color in (b"48;2;228;228;228", b"48;5;254"):
            with self.subTest(color=color):
                after = run.mark_frame()
                self.feed(run, BEGIN + b"\x1b[1;1H\x1b[" + color + b"m> default\x1b[0m" + END)
                self.assertTrue(run.screen.is_selected("default", after[1]))
        for color in (b"48;2;223;230;231", b"48;5;188", b"48;5;253"):
            with self.subTest(obsolete=color):
                after = run.mark_frame()
                self.feed(run, BEGIN + b"\x1b[1;1H\x1b[" + color + b"m> default\x1b[0m" + END)
                self.assertFalse(run.screen.is_selected("default", after[1]))
        after = run.mark_frame()
        self.feed(run, BEGIN + b"\x1b[1;1H\x1b[48;2;245;247;247m> default\x1b[0m" + END)
        self.assertFalse(run.screen.is_selected("default", after[1]), "page background is not selection")
        self.assertTrue(run.screen.is_marked("default", after[1]))

    def test_partial_selection_update_does_not_publish_new_highlight(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[1;1H\x1b[48;2;59;59;59mdefault\x1b[0m"
                  b"\x1b[2;1Haccept-edits" + END)
        after = run.mark_frame()
        self.feed(run, BEGIN + b"\x1b[1;1Hdefault\x1b[2;1H\x1b[48;2;59;59;59maccept-edits\x1b[0m")
        self.assertFalse(run.screen.is_selected("accept-edits", after[1]))
        self.feed(run, END)
        self.assertTrue(run.screen.is_selected("accept-edits", after[1]))
        self.assertFalse(run.screen.is_selected("default", after[1]))

    def test_no_color_selection_requires_current_leading_marker(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[1;1H> default\x1b[2;1H  accept-edits" + END)
        self.assertTrue(run.screen.is_marked("default", 0))
        self.assertFalse(run.screen.is_marked("accept-edits", 0))
        self.assertFalse(run.screen.is_selected("default", 0), "marker must not weaken color assertions")
        after = run.mark_frame()
        self.feed(run, BEGIN + b"\x1b[1;1H  default\x1b[2;1H> accept-edits")
        self.assertFalse(run.screen.is_marked("accept-edits", after[1]))
        self.feed(run, END)
        self.assertTrue(run.screen.is_marked("accept-edits", after[1]))
        self.assertFalse(run.screen.is_marked("default", after[1]))
        self.assertTrue(all(cell.fg == cell.bg == "default"
                            for row in run.screen.completed_cells for cell in row))

    def test_marker_in_text_or_another_row_cannot_select_a_label(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[1;1Hdefault > hint\x1b[2;1H> accept-edits" + END)
        self.assertFalse(run.screen.is_marked("default", 0))
        self.assertTrue(run.screen.is_marked("accept-edits", 0))
        run.screen.resize(lines=12, columns=40)
        self.assertFalse(run.screen.is_marked("accept-edits", 0))
        self.assertEqual(run.screen.completed_cells, ())

    def test_cell_styles_snapshot_only_completed_frames(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[38;2;220;220;220;48;2;59;59;59m> default\x1b[0m" + END)
        self.assertEqual(run.screen.completed_cells[0][0].fg, "dcdcdc")
        self.assertEqual(run.screen.completed_cells[0][0].bg, "3b3b3b")
        self.feed(run, BEGIN + b"\x1b[1;1H\x1b[0m> default")
        self.assertEqual(run.screen.completed_cells[0][0].bg, "3b3b3b")
        self.feed(run, END)
        self.assertEqual(run.screen.completed_cells[0][0].bg, "default")

    def test_theme_protocol_only_replies_to_observed_queries(self):
        for appearance, background in (("dark", b"0000/0000/0000"),
                                       ("light", b"ffff/ffff/ffff"),
                                       ("no_color", b"ffff/ffff/ffff")):
            with self.subTest(appearance=appearance):
                run = self.make_run()
                run.fixture = runtime.Fixture(appearance=appearance)
                run.answered = set()
                run.protocol_replies = run.background_replies = 0
                run.rows, run.columns = 24, 96
                run.send = mock.Mock()
                run.reply_to_queries()
                run.send.assert_not_called()
                self.feed(run, b"\x1b]11;?\x07")
                run.reply_to_queries()
                run.reply_to_queries()
                run.send.assert_called_once_with(b"\x1b]11;rgb:" + background + b"\x1b\\")
                self.assertEqual(run.background_replies, 1)

    def test_no_color_sgr_check_keeps_modifiers_but_rejects_color(self):
        runtime.assert_no_color(BEGIN + b"\x1b[1m> \x1b[4:1mquery\x1b[24m\x1b[39;49m\x1b[0m" + END)
        for color in (b"31", b"42", b"96", b"107", b"38;2;1;2;3", b"48;5;188", b"38:2::1:2:3"):
            with self.subTest(color=color), self.assertRaises(AssertionError):
                runtime.assert_no_color(BEGIN + b"\x1b[" + color + b"mtext" + END)

    def test_contrast_assertion_rejects_old_selected_metadata_color(self):
        self.assertLess(runtime.color_contrast("6b7280", "3b3b3b"), 4.5)
        self.assertGreaterEqual(runtime.color_contrast("c1c1c1", "3b3b3b"), 4.5)
        self.assertGreaterEqual(runtime.color_contrast("4a575a", "dfe6e7"), 4.5)
        self.assertGreaterEqual(runtime.color_contrast("525252", "e4e4e4"), 4.5)
        self.assertGreaterEqual(runtime.color_contrast("4e4e4e", "e4e4e4"), 4.5)

    def test_standard_256_palette_resolves_slt_neutral_and_semantic_roles(self):
        for appearance, indexes in (
                ("dark_256", (231, 250, 80, 210)),
                ("light_256", (234, 239, 23, 124))):
            run = self.make_run()
            for role, index in zip(("selection_text", "selection_muted", "accent", "danger"), indexes):
                with self.subTest(appearance=appearance, role=role):
                    self.feed(run, BEGIN + "\x1b[1;1H\x1b[38;5;{}mX".format(index).encode() + END)
                    self.assertEqual(run.screen.completed_cells[0][0].fg, runtime.COLOR_ROLES[appearance][role])
        self.assertEqual(232 + (228 - 8) * 24 // 240, 254)
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[48;5;254m> light" + END)
        self.assertEqual(run.screen.completed_cells[0][0].bg, "e4e4e4")

    @staticmethod
    def foreground(color):
        if color == "default":
            return b"\x1b[39m"
        if color in ("black", "white"):
            return b"\x1b[30m" if color == "black" else b"\x1b[37m"
        return "\x1b[38;2;{};{};{}m".format(
            *(int(color[index:index + 2], 16) for index in (0, 2, 4))).encode()

    def marker_screen(self, appearance):
        run = self.make_run()
        roles = runtime.COLOR_ROLES[appearance]
        self.feed(run, BEGIN + self.foreground(roles["selection_text"]) + b">* selected"
                  + b"\x1b[2;1H" + self.foreground(roles["selection_muted"]) + b" * pinned"
                  + b"\x1b[3;1H  2) menu" + END)
        return run

    def test_marker_roles_use_exact_selected_and_unselected_gray(self):
        for appearance in runtime.COLOR_ROLES:
            with self.subTest(appearance=appearance):
                run = self.marker_screen(appearance)
                self.assertEqual(runtime.assert_marker_roles(run.screen, appearance),
                                 {"pointer": 1, "pin": 2, "menu_number": 1})

    def test_marker_roles_reject_cyan_and_wrong_gray_for_each_role(self):
        for appearance in ("dark", "light", "dark_256", "light_256"):
            for row, column in ((1, 1), (1, 2), (2, 2), (3, 3), (3, 4)):
                for color in (runtime.COLOR_ROLES[appearance]["accent"], "808080"):
                    with self.subTest(appearance=appearance, row=row, column=column, color=color):
                        run = self.marker_screen(appearance)
                        cell = run.screen.completed_cells[row - 1][column - 1]
                        self.feed(run, BEGIN + "\x1b[{};{}H".format(row, column).encode()
                                  + self.foreground(color) + cell.data.encode() + END)
                        with self.assertRaises(AssertionError):
                            runtime.assert_marker_roles(run.screen, appearance)

    def test_marker_roles_check_every_visible_pointer_not_only_selected_prefix(self):
        run = self.marker_screen("dark")
        self.feed(run, BEGIN + b"\x1b[4;1H\x1b[38;2;88;205;207mother >" + END)
        with self.assertRaisesRegex(AssertionError, "pointer"):
            runtime.assert_marker_roles(run.screen, "dark")

    def test_marker_roles_read_completed_cells_not_inflight_updates(self):
        run = self.marker_screen("dark")
        self.feed(run, BEGIN + b"\x1b[1;1H\x1b[38;2;88;205;207m>")
        runtime.assert_marker_roles(run.screen, "dark")
        self.feed(run, END)
        with self.assertRaises(AssertionError):
            runtime.assert_marker_roles(run.screen, "dark")
        run.screen.resize(columns=40, lines=12)
        with self.assertRaises(AssertionError):
            runtime.assert_marker_roles(run.screen, "dark")

    def test_grouped_neutrality_rejects_brand_bleed_outside_agent_name(self):
        brands = {"claude": "e4a48c", "codex": "78bcaa"}
        for appearance in runtime.COLOR_ROLES:
            with self.subTest(appearance=appearance):
                colors = (dict.fromkeys(brands, runtime.COLOR_ROLES[appearance]["selection_text"])
                          if appearance == "no_color" or appearance in runtime.BASIC_APPEARANCES else brands)
                neutral = runtime.COLOR_ROLES[appearance]["selection_text"]
                run = self.make_run()
                prefix = "> \u2514\u2500* "
                self.feed(run, BEGIN + self.foreground(neutral) + prefix.encode()
                          + self.foreground(colors["codex"]) + b"codex"
                          + self.foreground(neutral) + b"  PTY_CODEX_SUMMARY  3m" + END)
                self.assertEqual(runtime.assert_grouped_roles(run.screen, appearance, colors), {"codex"})
                for column in (1, 3, 5, len(prefix) + len("codex  ") + 1, len(prefix) + len("codex  PTY_CODEX_SUMMARY  ") + 1):
                    cell = run.screen.completed_cells[0][column - 1]
                    self.feed(run, BEGIN + "\x1b[1;{}H".format(column).encode()
                              + self.foreground(brands["codex"]) + cell.data.encode() + END)
                    with self.assertRaises(AssertionError):
                        runtime.assert_grouped_roles(run.screen, appearance, colors)
                    self.feed(run, BEGIN + "\x1b[1;{}H".format(column).encode()
                              + self.foreground(neutral) + cell.data.encode() + END)

    def test_footer_accent_is_preserved_when_pointer_becomes_neutral(self):
        for appearance in runtime.COLOR_ROLES:
            with self.subTest(appearance=appearance):
                run = self.marker_screen(appearance)
                self.feed(run, BEGIN + b"\x1b[24;1H\x1b[1m"
                          + self.foreground(runtime.COLOR_ROLES[appearance]["accent"]) + b"Enter" + END)
                runtime.assert_footer_accent(run.screen, appearance)
                if appearance != "no_color" and appearance not in runtime.BASIC_APPEARANCES:
                    self.feed(run, BEGIN + b"\x1b[24;1H"
                              + self.foreground(runtime.COLOR_ROLES[appearance]["selection_text"]) + b"Enter" + END)
                    with self.assertRaises(AssertionError):
                        runtime.assert_footer_accent(run.screen, appearance)

    def test_basic_palette_requires_named_black_white_and_preserves_modifiers(self):
        for appearance, colors in (("dark_basic", b"37;40"), ("light_basic", b"30;47")):
            with self.subTest(appearance=appearance):
                run = self.make_run()
                painted_background = b"".join(
                    "\x1b[{};1H".format(row).encode() + b" " * 96 for row in range(1, 25))
                self.feed(run, BEGIN + b"\x1b[" + colors + b"m\x1b[2J" + painted_background
                          + b"\x1b[1;1H> \x1b[4:1mCodex\x1b[24m"
                          + b"\x1b[2;1H+ Settings saved\x1b[3;1H! Settings not saved"
                          + b"\x1b[24;1H\x1b[1mEnter\x1b[22m" + END)
                runtime.assert_basic_palette(run.screen, appearance, bytes(run.output))
                runtime.assert_marker_roles(run.screen, appearance)
                runtime.assert_footer_accent(run.screen, appearance)
                self.assertFalse(run.screen.is_selected("Codex", 0))
                self.assertTrue(run.screen.is_marked("Codex", 0))
                self.assertTrue(all(cell.underscore for cell in run.screen.completed_cells[0][2:7]))
                for sgr in (b"31", b"42", b"96", b"107", b"38;5;231", b"48;2;0;0;0", b"38:2::255:255:255"):
                    with self.subTest(sgr=sgr), self.assertRaises(AssertionError):
                        runtime.assert_basic_palette(run.screen, appearance, bytes(run.output) + b"\x1b[" + sgr + b"m")
                self.feed(run, BEGIN + b"\x1b[1;1H\x1b[39m>" + END)
                with self.assertRaises(AssertionError):
                    runtime.assert_basic_palette(run.screen, appearance, bytes(run.output))

    def test_basic_environment_cannot_accidentally_enable_truecolor_or_no_color(self):
        for appearance in runtime.BASIC_APPEARANCES:
            with self.subTest(appearance=appearance), runtime.Fixture(appearance=appearance) as fixture:
                environment = fixture.environment(123)
                self.assertEqual(environment["TERM"], "vt100")
                self.assertNotIn("COLORTERM", environment)
                self.assertNotIn("NO_COLOR", environment)
                if appearance == "light_basic":
                    self.assertIn('appearance = "light"', fixture.config.read_text(encoding="utf-8"))

    def test_codex_fixture_is_opt_in_and_included_in_history_immutability(self):
        for codex_brand in (False, True):
            with self.subTest(codex_brand=codex_brand), runtime.Fixture(ui_polish=True, codex_brand=codex_brand) as fixture:
                codex_root = fixture.home / ".codex"
                self.assertEqual(codex_root.exists(), codex_brand)
                self.assertEqual((fixture.bin / "codex").exists(), codex_brand)
                self.assertEqual(fixture.session_count, 3)
                fixture.assert_history_unchanged()
                if codex_brand:
                    history = codex_root / "history.jsonl"
                    self.assertIn(str(history.relative_to(fixture.root)), fixture.original_files)
                    with history.open("a", encoding="utf-8") as output:
                        output.write("{}\n")
                    with self.assertRaises(AssertionError):
                        fixture.assert_history_unchanged()

    def test_resizing_invalidates_selection_until_completed_redraw(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[1;1H\x1b[48;2;59;59;59m15) Antigravity\x1b[0m" + END)
        self.assertTrue(run.screen.is_selected("15) Antigravity", 0))
        run.screen.resize(lines=12, columns=40)
        self.assertFalse(run.screen.is_selected("15) Antigravity", 0))
        self.feed(run, BEGIN + b"\x1b[2J\x1b[1;1H15) Antigravity" + END)
        self.assertFalse(run.screen.is_selected("15) Antigravity", 0))

    def test_pyte_colon_underline_limitation_is_emulator_not_app_output(self):
        raw = runtime.FrameScreen(40, 12)
        pyte.ByteStream(raw).feed(BEGIN + b"\x1b[4:1m?[]\x1b[24m" + END)
        self.assertTrue(raw.completed[3].startswith("1m?[]"))
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[4:1m?[]\x1b[24m1m" + END)
        self.assertTrue(run.screen.completed[3].startswith("?[]1m"))
        self.assertTrue(run.screen.buffer[0][0].underscore)
        self.assertFalse(run.screen.buffer[0][3].underscore)
        self.assertIn(b"\x1b[4:1m", run.output, "raw evidence must never be normalized")

    def test_underline_compat_preserves_all_split_boundaries_and_utf8(self):
        text = "?[]" + runtime.TYPED
        data = BEGIN + b"\x1b[4:1m" + text.encode("utf-8") + b"\x1b[24m1m" + END
        for split in range(len(data) + 1):
            with self.subTest(split=split):
                run = self.make_run()
                self.feed(run, data[:split])
                self.feed(run, data[split:])
                self.assertTrue(run.screen.matches([text + "1m"], 0))
                self.assertTrue(run.screen.buffer[0][0].underscore)
        run = self.make_run()
        for byte in data:
            self.feed(run, bytes([byte]))
        self.assertTrue(run.screen.matches([text + "1m"], 0))
        self.assertEqual(run.screen.cursor.x, 5 + len(runtime.TYPED) * 2)

    def test_underline_compat_only_rewrites_exact_csi_outside_control_strings(self):
        unchanged = (b"literal 4:1m 1m ?[]\x1b[2;3H\x1b[38;2;1;2;3m"
                     b"\x1b[4:2m\x1b[4:1K\x1b[?2026h"
                     b"\x1b]2;title\x1b[4:1m\x07"
                     b"\x1bPpayload\x1b[4:1m\x1b\\")
        with mock.patch.object(pyte.ByteStream, "feed") as sink:
            stream = runtime.SltByteStream(runtime.FrameScreen(40, 12))
            for byte in unchanged + b"\x1b[4:1m":
                stream.feed(bytes([byte]))
            forwarded = b"".join(call.args[0] for call in sink.call_args_list)
        self.assertEqual(forwarded, unchanged + b"\x1b[4m")

    def test_underline_compat_does_not_publish_incomplete_sync_frame(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[4:")
        self.assertIsNone(run.screen.completed)
        self.feed(run, b"1m?[]\x1b[24m" + END[:-1])
        self.assertIsNone(run.screen.completed)
        self.feed(run, END[-1:])
        self.assertTrue(run.screen.matches(["?[]"], 0))

    def test_underline_compat_bounds_pending_sequence_without_dropping_bytes(self):
        data = b"\x1b[" + b"1;" * 200 + b"m1m"
        with mock.patch.object(pyte.ByteStream, "feed") as sink:
            stream = runtime.SltByteStream(runtime.FrameScreen(40, 12))
            for byte in data:
                stream.feed(bytes([byte]))
                self.assertLess(len(stream._pending_csi), 64)
            forwarded = b"".join(call.args[0] for call in sink.call_args_list)
        self.assertEqual(forwarded, data)

    def test_footer_keys_must_be_whole_and_on_the_last_row(self):
        run = self.make_run()
        self.feed(run, BEGIN + b"\x1b[1;1HEnter F1 Esc\x1b[24;1HEnter F10 Escaped" + END)
        self.assertFalse(run.screen.footer_matches(["Enter", "F1", "Esc"], 0))
        self.feed(run, BEGIN + b"\x1b[24;1H\x1b[2KEnter F1 Esc" + END)
        self.assertTrue(run.screen.footer_matches(["Enter", "F1", "Esc"], 0))
        after = run.mark_frame()
        self.feed(run, BEGIN + b"\x1b[24;1H\x1b[2KEnter F1 Es")
        self.assertFalse(run.screen.footer_matches(["Enter", "F1", "Esc"], after[1]))
        self.feed(run, END)
        self.assertFalse(run.screen.footer_matches(["Enter", "F1", "Esc"], 0))

    def test_footer_geometry_and_selection_survive_each_supported_width(self):
        run = self.make_run()
        for width in (20, 39, 40, 80, 120):
            with self.subTest(width=width):
                run.screen.resize(lines=12, columns=width)
                self.assertFalse(run.screen.footer_matches(["Enter", "F1", "Esc"], 0))
                self.feed(run, BEGIN + b"\x1b[2J\x1b[4;1H\x1b[48;5;237mPTY_LITERAL_?[]"
                          b"\x1b[0m\x1b[12;1HEnter F1 Esc" + END)
                self.assertEqual(run.screen.completed[2], (width, 12))
                self.assertTrue(run.screen.footer_matches(["Enter", "F1", "Esc"], 0))
                self.assertTrue(run.screen.is_selected("PTY_LITERAL_?[]", 0))
                self.assertEqual(run.screen.completed[3].splitlines()[-1].rstrip(), "Enter F1 Esc")


if __name__ == "__main__":
    unittest.main(verbosity=2)
