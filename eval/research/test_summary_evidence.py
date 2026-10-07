import copy
import json
import unittest
import hashlib

from import_hotpot import convert
from summary_evidence import document_coverage, literal_link_coverage


class EvidenceTest(unittest.TestCase):
    def test_date_link_coverage_is_group_bound_and_never_entailment(self):
        groups=[{"id":"identity","text":"Ada is a member of Beta.\n"},
                {"id":"date","text":"Ada joined Beta in 2019.\n"},
                {"id":"noise","text":"An unrelated event happened in 2019.\n"}]
        task={"source":"".join(g["text"] for g in groups),"query":"When did Ada join Beta?",
              "select_context":{"groups":groups}}
        annotations={"source_sha256":hashlib.sha256(task["source"].encode()).hexdigest(),
                     "query_sha256":hashlib.sha256(task["query"].encode()).hexdigest(),
                     "links":[{"id":"identity-link","group_id":"identity","literal":"Ada is a member of Beta."},
                              {"id":"date-link","group_id":"date","literal":"Ada joined Beta in 2019."}]}
        before=copy.deepcopy(task)
        def frame(indexes):
            return json.dumps({"summary_unverified":"Ada joined in 2020 (unsupported).",
                               "source_evidence":[groups[i] for i in indexes]})
        missing=literal_link_coverage(task,annotations,frame([0,2]))
        self.assertEqual(missing["missing_link_ids"],["date-link"])
        self.assertEqual(missing["literal_link_recall"],.5)  # Same year in another group is not the missing link.
        complete=literal_link_coverage(task,annotations,frame([0,1]))
        self.assertEqual(complete["literal_link_recall"],1)
        self.assertIn("does not prove summary entailment",complete["limitation"])
        self.assertEqual(literal_link_coverage(task,annotations,task["source"])["literal_link_recall"],1)
        self.assertIsNone(literal_link_coverage(task,annotations,"Narrative, not a native frame"))
        for changes in ({"query_sha256":"0"*64},{"source_sha256":"0"*64},
                        {"links":annotations["links"]*2},
                        {"links":[{"id":"bad","group_id":"date","literal":"2020"}]}):
            with self.assertRaises(ValueError):
                literal_link_coverage(task,{**annotations,**changes},frame([0,1]))
        changed=json.loads(frame([0,1]));changed["source_evidence"][1]["text"]="2019"
        with self.assertRaises(ValueError): literal_link_coverage(task,annotations,json.dumps(changed))
        self.assertEqual(task,before)

    def setUp(self):
        self.record = {"_id": "case", "question": "Where is Company A's airport?", "answer": "Town B",
                       "context": [["Company", ["Company A operates Airport C."]],
                                   ["Airport", ["Airport C is in Town B."]], ["Noise", ["Unrelated."]]],
                       "supporting_facts": [["Company", 0], ["Airport", 0]]}
        self.task = convert([self.record])[0]

    def payload(self, indexes, summary="Airport C is in Town B"):
        return json.dumps({"summary_unverified": summary,
                           "source_evidence": [self.task["select_context"]["groups"][i] for i in indexes]})

    def test_literal_coverage_never_certifies_generated_truth(self):
        complete = document_coverage(self.task, self.record, self.payload([0, 1], "Invented claim"))
        self.assertEqual(complete["supporting_document_recall"], 1)
        self.assertEqual(complete["supporting_document_precision"], 1)
        self.assertIn("not summary entailment", complete["limitation"])
        missing = document_coverage(self.task, self.record, self.payload([0, 2]))
        self.assertEqual(missing["supporting_document_recall"], .5)
        self.assertEqual(missing["supporting_document_precision"], .5)
        raw = document_coverage(self.task, self.record, self.task["source"])
        self.assertEqual(raw["kind"], "raw_literal_source")
        self.assertEqual(raw["supporting_document_recall"], 1)
        self.assertIsNone(document_coverage(self.task, self.record, "Refined narrative observation"))
        self.assertIsNone(document_coverage(self.task, self.record, '{"different_style":true}'))
        columns = {**self.record, "context": {"title": [r[0] for r in self.record["context"]],
                                              "sentences": [r[1] for r in self.record["context"]]},
                   "supporting_facts": {"title": ["Company", "Airport"], "sent_id": [0, 0]}}
        self.assertEqual(document_coverage(self.task, columns, self.payload([0, 1])),
                         document_coverage(self.task, self.record, self.payload([0, 1])))

    def test_authorization_labels_and_duplicates_fail_closed_without_mutation(self):
        before = copy.deepcopy(self.task)
        for mutation in (lambda d: d["source_evidence"][0].update(text="changed"),
                         lambda d: d["source_evidence"][0].update(id="unknown")):
            value = json.loads(self.payload([0])); mutation(value)
            with self.assertRaises(ValueError):
                document_coverage(self.task, self.record, json.dumps(value))
        for payload in (self.payload([0, 0]), '{"summary_unverified":"a","summary_unverified":"b","source_evidence":[]}'):
            with self.assertRaises(ValueError):
                document_coverage(self.task, self.record, payload)
        for record in ({**self.record, "supporting_facts": []},
                       {**self.record, "supporting_facts": [["Airport", True]]},
                       {**self.record, "supporting_facts": [["Airport", 1]]},
                       {**self.record, "supporting_facts": {"title": ["Airport"], "sent_id": []}},
                       {**self.record, "question": "other query"}):
            with self.assertRaises(ValueError):
                document_coverage(self.task, record, self.payload([0]))
        self.assertEqual(self.task, before)  # Gold never changes source/protection/prompt input.


if __name__ == "__main__":
    unittest.main()
