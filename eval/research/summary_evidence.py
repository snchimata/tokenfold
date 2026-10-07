"""Post-hoc Hotpot native literal-document coverage, not semantic verification.

Human supporting-fact labels stay evaluator-only: never alter caller grouping,
protection, selection or model prompts. Unsupported narrative styles score null,
not zero. This is NOT the official sentence-level supporting-fact/joint metric.
"""
import json
import hashlib

from import_hotpot import convert


def literal_link_coverage(task, annotations, payload):
    """Evaluator-only source/query-bound link labels; never selection or admission."""
    from select_observation import source_context
    from grounded_answer import EvidenceSummaryJudge
    if (not isinstance(annotations, dict) or set(annotations) != {"source_sha256", "query_sha256", "links"}
            or len(json.dumps(annotations).encode()) > 65536
            or annotations["source_sha256"] != hashlib.sha256(task["source"].encode()).hexdigest()
            or annotations["query_sha256"] != hashlib.sha256(task["query"].encode()).hexdigest()):
        raise ValueError("link annotation source/query mismatch")
    groups = source_context(task)["groups"]
    by_id = {group["id"]: group["text"] for group in groups}
    links = annotations["links"]
    if not isinstance(links, list) or not 1 <= len(links) <= 512:
        raise ValueError("bounded nonempty literal links required")
    seen, spans = set(), set()
    for link in links:
        if (not isinstance(link, dict) or set(link) != {"id", "group_id", "literal"}
                or any(not isinstance(link[field], str) or not link[field] for field in link)
                or link["id"] in seen or link["group_id"] not in by_id
                or link["literal"] not in by_id[link["group_id"]]
                or (link["group_id"], link["literal"]) in spans):
            raise ValueError("invalid or duplicate source-bound link label")
        seen.add(link["id"]); spans.add((link["group_id"], link["literal"]))
    if not isinstance(payload, str) or len(payload.encode()) > 1024 * 1024:
        raise ValueError("invalid evidence payload")
    if payload == task["source"]:
        selected = set(by_id)
    else:
        try:
            _, _, _, selected, _ = EvidenceSummaryJudge._validated_frame(task, payload)
        except json.JSONDecodeError:
            return None  # Unsupported prose is not a measured retention failure.
        selected = set(selected) | {group["id"] for group in groups if group.get("required", False)}
    missing = [link["id"] for link in links if link["group_id"] not in selected]
    return {"kind": "evaluator-only-literal-link-coverage", "required_links": len(links),
            "retained_links": len(links) - len(missing), "missing_link_ids": missing,
            "literal_link_recall": (len(links) - len(missing)) / len(links),
            "limitation": "Human link labels are evaluator-only and may be incomplete/wrong; literal retention "
                          "does not prove summary entailment, semantic link completeness, answer quality or admission."}


def unique_object(pairs):
    value = dict(pairs)
    if len(value) != len(pairs):
        raise ValueError("duplicate_evidence_field")
    return value


def document_coverage(task, upstream_record, payload):
    expected = convert([upstream_record])[0]
    if any(task.get(key) != expected[key] for key in ("id", "source", "query", "select_context")):
        raise ValueError("source_record_mismatch")
    context = upstream_record["context"]
    if isinstance(context, dict):
        context = list(zip(context["title"], context["sentences"]))
    if len({title for title, _ in context}) != len(context):
        raise ValueError("ambiguous_document_titles")
    paragraphs = dict(context)
    facts = upstream_record.get("supporting_facts")
    if isinstance(facts, dict):
        titles, indexes = facts.get("title"), facts.get("sent_id")
        if not isinstance(titles, list) or not isinstance(indexes, list) or len(titles) != len(indexes):
            raise ValueError("supporting_fact_columns_mismatch")
        facts = list(zip(titles, indexes))
    if not isinstance(facts, list) or not facts:
        raise ValueError("supporting_fact_labels_missing")
    for fact in facts:
        if (not isinstance(fact, (list, tuple)) or len(fact) != 2
                or not isinstance(fact[0], str) or fact[0] not in paragraphs
                or type(fact[1]) is not int or not 0 <= fact[1] < len(paragraphs[fact[0]])):
            raise ValueError("invalid_supporting_fact_label")
    required = {title for title, _ in facts}
    groups = expected["select_context"]["groups"]
    if not isinstance(payload, str) or len(payload.encode()) > 1024 * 1024:
        raise ValueError("invalid_evidence_payload")
    if payload == task["source"]:
        selected = set(range(len(groups)))
        kind = "raw_literal_source"
    else:
        try:
            wrapper = json.loads(payload, object_pairs_hook=unique_object)
        except json.JSONDecodeError:
            return None  # Unsupported prose/frames are not evidence-retention losses.
        if not isinstance(wrapper, dict) or set(wrapper) != {"summary_unverified", "source_evidence"}:
            return None
        evidence = wrapper["source_evidence"]
        if not isinstance(wrapper["summary_unverified"], str) or not isinstance(evidence, list):
            raise ValueError("invalid_native_evidence")
        by_id = {g["id"]: (i, g["text"]) for i, g in enumerate(groups)}
        selected = set()
        for item in evidence:
            if (not isinstance(item, dict) or set(item) != {"id", "text"}
                    or not isinstance(item["id"], str) or item["id"] not in by_id
                    or item["text"] != by_id[item["id"]][1]):
                raise ValueError("unauthorized_or_changed_evidence")
            index = by_id[item["id"]][0]
            if index in selected:
                raise ValueError("duplicate_source_evidence")
            selected.add(index)
        kind = "native_literal_groups"
    selected_titles = {context[i][0] for i in selected}
    retained = len(required & selected_titles)
    return {"kind": kind, "required_documents": len(required), "selected_documents": len(selected),
            "retained_required_documents": retained, "supporting_document_recall": retained / len(required),
            "supporting_document_precision": retained / len(selected) if selected else 0,
            "limitation": "Literal document coverage only; not summary entailment, answer quality, "
            "official sentence-level supporting-fact/joint evaluation or independent qualification."}
