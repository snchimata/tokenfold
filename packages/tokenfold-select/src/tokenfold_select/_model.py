"""Development-only joint group encoder. Random head: NOT a usable Select scorer.

No downloads, truncation, windowing, generated text or production integration.
The caller owns request validation and supplies the provisioned encoder/tokenizer.
"""
import torch
from torch import nn


def pack_groups(tokenizer, query, groups, cap=8192):
    """Return one sequence and explicit content ranges in caller order."""
    if not 1 <= cap <= 8192 or not query or not 1 <= len(groups) <= 512:
        raise ValueError("invalid_admission")
    ids_seen = set()
    cls, sep = tokenizer.cls_token_id, tokenizer.sep_token_id
    if cls is None or sep is None:
        raise ValueError("missing_special_tokens")
    ids = [cls] + tokenizer.encode(query, add_special_tokens=False, truncation=False) + [sep]
    ranges = []
    for group in groups:
        gid, text = group['id'], group['text']
        if not isinstance(gid, str) or not gid or gid in ids_seen or not isinstance(text, str) or not text:
            raise ValueError("invalid_group")
        ids_seen.add(gid)
        content = tokenizer.encode(text, add_special_tokens=False, truncation=False)
        if not content:
            raise ValueError("empty_group_range")
        start = len(ids)
        ids.extend(content)
        ranges.append((start, len(ids)))
        ids.append(sep)
        if len(ids) > cap:
            raise ValueError("token_cap")
    return ids, ranges


class JointGroupModel(nn.Module):
    """One encoder invocation, masked content mean and a shared linear head."""

    def __init__(self, encoder):
        super().__init__()
        self.encoder = encoder
        self.head = nn.Linear(encoder.config.hidden_size, 1)

    def forward(self, input_ids, attention_mask, ranges):
        if input_ids.ndim != 2 or input_ids.shape[0] != 1 or attention_mask.shape != input_ids.shape:
            raise ValueError("single_sequence_required")
        previous = 0
        if not 1 <= len(ranges) <= 512:
            raise ValueError("invalid_ranges")
        for start, end in ranges:
            if not previous <= start < end <= input_ids.shape[1] or not attention_mask[0, start:end].all():
                raise ValueError("invalid_ranges")
            previous = end
        positions = torch.arange(input_ids.shape[1], device=input_ids.device).unsqueeze(0)
        states = self.encoder(input_ids=input_ids, attention_mask=attention_mask,
                              position_ids=positions).last_hidden_state[0]
        pooled = torch.stack([states[start:end].mean(0) for start, end in ranges])
        scores = self.head(pooled).squeeze(-1)
        if not torch.isfinite(scores).all():
            raise ValueError("nonfinite_scores")
        return scores
