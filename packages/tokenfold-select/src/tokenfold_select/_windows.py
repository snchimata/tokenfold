"""Bounded source-conditioned windows, shared by future training and inference.

Original groups are never cropped or split. A bounded auxiliary source-tail copy
is unscored encoder context, not extra retained text or consumer input.
"""

import hashlib
from pathlib import Path

PROFILE = "source-tail-1536-capacity-v2"
CONTEXT_TOKENS = 1536
WINDOW_TOKENS = 8192
MAX_GROUPS = 512
MAX_WINDOWS = 64
MAX_TOTAL_TOKENS = MAX_WINDOWS * WINDOW_TOKENS


def code_hash():
    return hashlib.sha256(Path(__file__).read_bytes()).hexdigest()


def verify_profile(report):
    recipe = report.get("recipe", {})
    if (recipe.get("scoring_window_profile") != PROFILE
            or recipe.get("scoring_context_token_cap") != CONTEXT_TOKENS
            or recipe.get("scoring_window_code_sha256") != code_hash()):
        raise ValueError("matching_trained_source_context_profile_required")


def pack_windows(tokenizer, config, request):
    """Return legacy-shaped windows and a non-qualifying packing receipt.

    Tokenization is once per query/group. Each original group has one scored
    range; auxiliary copies have no ranges. All admission finishes before forward.
    """
    groups = request["groups"]
    if not isinstance(groups, list) or not 1 <= len(groups) <= MAX_GROUPS:
        raise ValueError("bounded_nonempty_groups_required")
    if (not isinstance(request.get("query"), str) or not request["query"].strip()
            or any(not isinstance(g, dict) or not isinstance(g.get("text"), str) or not g["text"] for g in groups)):
        raise ValueError("original_query_and_group_text_required")
    tokenizer.no_truncation(); tokenizer.no_padding()
    def special(name):
        value = config[name]
        token = tokenizer.token_to_id(value["content"] if isinstance(value, dict) else value)
        if token is None: raise ValueError("missing_special_token")
        return token
    cls, sep = special("cls_token"), special("sep_token")
    query = [cls] + tokenizer.encode(request["query"], add_special_tokens=False).ids + [sep]
    if len(query) >= WINDOW_TOKENS: raise ValueError("query_token_cap")
    encoded = [tokenizer.encode(group["text"], add_special_tokens=False).ids for group in groups]
    if any(not ids or len(query) + len(ids) + 1 > WINDOW_TOKENS for ids in encoded):
        raise ValueError("whole_group_token_cap")
    source_tokens = sum(len(ids) + 1 for ids in encoded)
    if source_tokens + len(query) > MAX_TOTAL_TOKENS:
        raise ValueError("total_token_cap")
    context = []; capacity_checks = 0; compatible_allowance = 0; reduced = False
    if source_tokens + len(query) > WINDOW_TOKENS:
        allowance = min(CONTEXT_TOKENS, WINDOW_TOKENS - len(query) - max(map(len, encoded)) - 2)
        if allowance < 1: raise ValueError("whole_group_leaves_no_source_context")
        compatible_allowance = allowance
        def demand(amount):
            nonlocal capacity_checks
            capacity_checks += 1
            header = len(query) + amount + 1
            count = 1; used = header
            for content in encoded:
                length = len(content) + 1
                if used + length > WINDOW_TOKENS:
                    count += 1; used = header
                used += length
            return count, source_tokens + count * header
        count, total = demand(allowance)
        if count > MAX_WINDOWS or total > MAX_TOTAL_TOKENS:
            count, total = demand(1)
            if count > MAX_WINDOWS: raise ValueError("window_count_cap")
            if total > MAX_TOTAL_TOKENS: raise ValueError("total_token_cap")
            # Largest feasible auxiliary copy, never fewer original source tokens.
            low, high = 1, allowance
            while low < high:
                mid = (low + high + 1) // 2
                count, total = demand(mid)
                if count <= MAX_WINDOWS and total <= MAX_TOTAL_TOKENS: low = mid
                else: high = mid - 1
            allowance = low; reduced = True
        for ids in encoded:
            context = (context + [sep] + ids)[-allowance:]
    header = query + context + [sep] if context else query
    windows = []
    ids, ranges, indices = list(header), [], []
    for index, content in enumerate(encoded):
        if len(ids) + len(content) + 1 > WINDOW_TOKENS:
            windows.append((ids, ranges, indices))
            if len(windows) >= MAX_WINDOWS: raise ValueError("window_count_cap")
            ids, ranges, indices = list(header), [], []
        start = len(ids); ids.extend(content); ids.append(sep)
        ranges.append((start, start + len(content))); indices.append(index)
    windows.append((ids, ranges, indices))
    total = sum(len(window[0]) for window in windows)
    if total > MAX_TOTAL_TOKENS: raise ValueError("total_token_cap")
    return windows, {"profile": PROFILE, "context_tokens_per_window": len(context),
                     "max_context_tokens": CONTEXT_TOKENS,
                     "whole_group_compatible_context_tokens": compatible_allowance,
                     "context_reduced_for_capacity": reduced, "capacity_checks": capacity_checks,
                     "scored_source_tokens_with_separators": source_tokens,
                     "total_encoded_tokens_including_copies": total,
                     "windows": len(windows), "groups": len(groups), "qualification": False}
