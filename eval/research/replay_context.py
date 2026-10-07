"""Explicit text-history replay contract, not an agent/tool-protocol implementation."""


def replay_source(messages: list[dict], observation: str) -> str:
    if (not isinstance(messages, list) or not messages or len(messages) > 64
            or not isinstance(observation, str) or not observation):
        raise ValueError("bounded nonempty history and observation required")
    for message in messages:
        if (not isinstance(message, dict) or set(message) != {"role", "content"}
                or message["role"] not in {"user", "assistant"}
                or not isinstance(message["content"], str) or not message["content"]):
            raise ValueError("replay supports text user/assistant messages only")
    return "".join(m["role"].upper() + ":\n" + m["content"] + "\n" for m in messages) + "OBSERVATION:\n" + observation


def validate_replay(task: dict):
    messages = task.get("history_messages")
    if messages is None:
        if "observation" in task:
            raise ValueError("observation requires explicit history_messages")
        return
    if replay_source(messages, task.get("observation")) != task["source"]:
        raise ValueError("replayed source differs from declared history/observation")
