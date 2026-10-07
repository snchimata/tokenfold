import copy
import json
import tempfile
import unittest
from pathlib import Path
import import_usgs as usgs
from select_observation import source_context


class UsgsTests(unittest.TestCase):
    def test_multistep_selection_uses_thresholds_time_ties_and_explicit_no_match(self):
        document = self.fixture()
        for i, feature in enumerate(document["features"]):
            feature["properties"].update(mag=5, time=100+i)
        original = copy.deepcopy(document)
        task = usgs.convert(document, "window", "latest-shallow-moderate")
        self.assertEqual(task["gold_answer"], "us2")
        self.assertEqual(document, original)
        self.assertEqual(json.loads(task["source"]), document)
        source_context(task)
        document["features"][2]["geometry"]["coordinates"][2] = 71
        self.assertEqual(usgs.convert(document, "window", "latest-shallow-moderate")["gold_answer"], "us1")
        document["features"][1]["properties"]["time"] = 100
        self.assertEqual(usgs.convert(document, "window", "latest-shallow-moderate")["gold_answer"], "us0")
        document["features"][0]["properties"]["mag"] = 4.49
        document["features"][1]["properties"]["mag"] = None
        self.assertEqual(usgs.convert(document, "window", "latest-shallow-moderate")["gold_answer"], "NONE")
        document["features"][1]["properties"]["mag"] = True
        with self.assertRaises(ValueError):
            usgs.convert(document, "window", "latest-shallow-moderate")
        self.assertNotIn("gold_answer", json.dumps(task["select_context"]))

    def fixture(self):
        return {"type":"FeatureCollection","metadata":{"count":3},"features":[
            {"type":"Feature","id":f"us{i}","properties":{"net":"us","place":f"Place {i}"},
             "geometry":{"type":"Point","coordinates":[-1,2,0.01]}} for i in range(3)],"bbox":[-1,2,0.01]}

    def test_source_only_grouping_and_exact_response(self):
        document=self.fixture();original=copy.deepcopy(document)
        task=usgs.convert(document,"time-window")
        self.assertEqual(document,original)
        self.assertEqual(json.loads(task["source"]),document)
        self.assertEqual(task["gold_answer"],"Place 1")
        self.assertEqual(task["critical_atoms"],[])
        source_context(task)
        self.assertFalse(any(g.get("required") for g in task["select_context"]["groups"]))
        document["features"][1]["id"]="us0"
        with self.assertRaises(ValueError):usgs.convert(document,"time-window")

    def test_fingerprints_overlap_and_exclusive_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);data=json.dumps(self.fixture()).encode();(root/"source.json").write_bytes(data)
            entry={"path":"source.json","sha256":usgs.digest(data),"cluster_id":"window", "split":"train",
                   "url":"https://earthquake.usgs.gov/fdsnws/event/1/query?format=geojson&contributor=us"}
            manifest=root/"manifest.json";manifest.write_text(json.dumps({"files":[entry]}))
            args=["--snapshot",str(root),"--manifest",str(manifest),"--output-dir",str(root/"output")]
            self.assertEqual(usgs.main(args),0)
            with self.assertRaises(FileExistsError):usgs.main(args)
            (root/"other.json").write_bytes(data)
            duplicate={**entry,"path":"other.json","cluster_id":"other","split":"test"}
            manifest.write_text(json.dumps({"files":[entry,duplicate]}))
            with self.assertRaises(ValueError):usgs.main(args)
            entry["sha256"]="0"*64;manifest.write_text(json.dumps({"files":[entry]}))
            with self.assertRaises(ValueError):usgs.main(args)


    def test_aliases_cannot_hide_same_event_under_new_canonical_id(self):
        first=self.fixture()
        first["features"][0]["properties"]["ids"]=",us0, alternate-id,"
        self.assertEqual(json.loads(usgs.convert(first,"first")["source"]),first)
        within=copy.deepcopy(first)
        within["features"][1]["properties"]["ids"]=",alternate-id,"
        with self.assertRaises(ValueError):usgs.convert(within,"within")
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);entries=[]
            second=copy.deepcopy(first)
            for feature in second["features"]:feature["id"]="new-"+feature["id"]
            for name,document,split in (("first",first,"train"),("second",second,"test")):
                data=json.dumps(document).encode();(root/(name+".json")).write_bytes(data)
                entries.append({"path":name+".json","sha256":usgs.digest(data),"cluster_id":name,"split":split,
                                "url":"https://earthquake.usgs.gov/fdsnws/event/1/query?format=geojson&contributor=us"})
            manifest=root/"manifest.json";manifest.write_text(json.dumps({"files":entries}))
            with self.assertRaises(ValueError):usgs.main(["--snapshot",str(root),"--manifest",str(manifest),"--output-dir",str(root/"output")])
            self.assertFalse((root/"output").exists())


if __name__=="__main__":unittest.main()
