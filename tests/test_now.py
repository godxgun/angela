import os
import tempfile
import unittest
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import now


class TestNow(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        now.DB_PATH = Path(self.tmp.name) / 'journal.sqlite'
        self.db = now.db()

    def tearDown(self):
        self.db.close()
        self.tmp.cleanup()

    def test_deadline_sort(self):
        now.add_deadline(self.db, 'VI project', '2026-10-19T23:59')
        now.add_deadline(self.db, 'DJ project', '2026-10-19T14:00')
        self.assertEqual([d['title'] for d in now.deadlines(self.db)], ['DJ project', 'VI project'])

    def test_session_lifecycle(self):
        now.record_start(self.db, 'Docs')
        self.assertEqual(now.active_session(self.db)['project'], 'Docs')
        now.record_stop(self.db)
        self.assertIsNone(now.active_session(self.db))

    def test_retroactive_log_preserves_text_offline(self):
        now.add_log(self.db, 'Worked all afternoon on game dev', use_llm=False)
        row = self.db.execute('SELECT raw FROM journal').fetchone()
        self.assertEqual(row['raw'], 'Worked all afternoon on game dev')
        self.assertEqual(self.db.execute('SELECT count(*) AS n FROM journal_items').fetchone()['n'], 0)

    def test_parse_google_tsv(self):
        src = ('2026-10-19\t18:00\t2026-10-19\t20:00\turl\tAula - DJ\troom\t\n'
               '2026-10-19\t\t2026-10-20\t\turl\tEntrega de trabalhos - DJ\t\tHora limite: 14:00\n')
        r = now.parse_agenda_tsv(src)
        self.assertEqual(len(r), 2)
        self.assertEqual(r[0]['when'].hour, 14)
        self.assertTrue(r[0]['all_day'])

    def test_priority_from_urgent_deadline(self):
        t = now.dt('2026-10-18T12:00')
        now.add_deadline(self.db, 'VI project', '2026-10-19T23:59')
        self.assertIn('VI project', now.priority(self.db, t, []))

    def test_reported_minutes_counts_for_plan_not_twice(self):
        t = now.current_time()
        row = self.db.execute('INSERT INTO journal(created, raw) VALUES (?, ?)', (now.stamp(t), 'Drew for 120 minutes'))
        self.db.execute('INSERT INTO journal_items(journal_id, project, minutes) VALUES (?, ?, ?)', (row.lastrowid, 'Drawing', 120))
        self.db.commit()
        self.assertEqual(now.recent_totals(self.db)['reported_today']['Drawing'], 120)


if __name__ == '__main__':
    unittest.main()

class TestAdaptiveState(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        now.DB_PATH = Path(self.tmp.name) / 'journal.sqlite'
        self.db = now.db()

    def tearDown(self):
        self.db.close()
        self.tmp.cleanup()

    def test_snapshot_returns_clean_json_contract(self):
        from unittest.mock import patch
        fixed = now.dt('2026-10-18T10:00')
        now.add_deadline(self.db, 'DJ deadline', '2026-10-19T14:00')
        with patch.object(now, 'cached_agenda', return_value=([], 'not connected')):
            state = now.snapshot(self.db, fixed)
        self.assertEqual(state['recommendation']['project'], 'DJ')
        self.assertEqual(state['nearest_deadline']['title'], 'DJ deadline')
        self.assertIn('calendar_error', state)
        import json
        json.dumps(state)

    def test_live_class_blocks_recommendation(self):
        fixed = now.dt('2026-10-21T10:00')
        agenda = [{'when': now.dt('2026-10-21T09:00'),
                   'end': now.dt('2026-10-21T11:00'),
                   'title': 'Aula - VI', 'all_day': False}]
        state = now.structured_recommendation(self.db, fixed, agenda)
        self.assertIsNone(state['project'])
        self.assertEqual(state['short'], 'In class / event')

    def test_sleep_suppresses_work(self):
        fixed = now.current_time()
        now.record_start(self.db, 'Sleep', at=fixed)
        state = now.structured_recommendation(self.db, fixed, [])
        self.assertIsNone(state['project'])

    def test_recommendation_balances_docs_games_when_daily_goals_met(self):
        fixed = now.current_time()
        for project, minutes in [('DI', 60), ('DJ', 60), ('VI', 60), ('DS', 60),
                                 ('Drawing', 120), ('Cardio', 60), ('Weightlifting', 60),
                                 ('Docs', 180), ('Game development', 60)]:
            at = fixed - now.timedelta(minutes=minutes)
            self.db.execute('INSERT INTO sessions(project,started,ended) VALUES (?,?,?)',
                            (project, now.stamp(at), now.stamp(fixed)))
        self.db.commit()
        state = now.structured_recommendation(self.db, fixed, [])
        self.assertEqual(state['project'], 'Game development')

    def test_tsv_end_used_to_detect_active_class(self):
        tsv = '2026-10-21\t09:00\t2026-10-21\t11:00\turl\tAula - VI\troom\t\n'
        rows = now.parse_agenda_tsv(tsv)
        self.assertEqual(rows[0]['end'].hour, 11)

class TestGoogleBridge(unittest.TestCase):
    def test_tries_new_then_legacy_gcalcli_syntax(self):
        from unittest.mock import patch
        from types import SimpleNamespace
        sample = '2026-10-19\t09:00\t2026-10-19\t10:00\turl\tAula - DI\troom\t\n'
        fake = [SimpleNamespace(returncode=2, stderr='unsupported argument', stdout=''),
                SimpleNamespace(returncode=0, stderr='', stdout=sample)]
        with patch.object(now.shutil, 'which', return_value='/usr/bin/gcalcli'), \
             patch.object(now.subprocess, 'run', side_effect=fake) as runner:
            result, err = now.google_agenda(now.dt('2026-10-18T12:00'))
        self.assertIsNone(err)
        self.assertEqual(len(result), 1)
        self.assertIn('agenda', runner.call_args_list[0].args[0])
        self.assertEqual(runner.call_count, 2)
