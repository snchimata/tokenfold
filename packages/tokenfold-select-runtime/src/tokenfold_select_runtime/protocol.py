"""Existing Select schema one, with bounded fail-closed JSON handling."""
import json
import math

REQUEST_BYTES = 1024 * 1024
RESPONSE_BYTES = 65536


def _unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError('duplicate_JSON_key')
        value[key] = item
    return value


def _number(text):
    value = float(text)
    if not math.isfinite(value):
        raise ValueError('nonfinite_JSON')
    return value


def _constant(_):
    raise ValueError('nonfinite_JSON')


def decode(data):
    return json.loads(data, object_pairs_hook=_unique, parse_float=_number, parse_constant=_constant)


def load_request(data, revision):
    if not isinstance(data, bytes) or len(data) > REQUEST_BYTES:
        raise ValueError('request_byte_limit')
    request = decode(data)
    return validate_request(request, revision)


def validate_request(request, revision):
    if (not isinstance(request, dict)
            or set(request) != {'schema_version', 'model_revision', 'query', 'groups'}
            or type(request['schema_version']) is not int or request['schema_version'] != 1
            or request['model_revision'] != revision
            or not isinstance(request['query'], str) or not request['query'].strip()
            or len(request['query'].encode('utf8')) > 4096):
        raise ValueError('invalid_select_request')
    groups = request['groups']
    if (not isinstance(groups, list) or not 1 <= len(groups) <= 512
            or any(not isinstance(g, dict) or set(g) != {'id', 'text'}
                   or not isinstance(g['id'], str) or not g['id'] or len(g['id'].encode('utf8')) > 256
                   or not isinstance(g['text'], str) or not g['text'] for g in groups)
            or len({g['id'] for g in groups}) != len(groups)):
        raise ValueError('invalid_groups')
    if len(json.dumps(request, ensure_ascii=False, allow_nan=False).encode('utf8')) > REQUEST_BYTES:
        raise ValueError('request_byte_limit')
    return request


def encode_response(request, scores):
    if (not isinstance(scores, list) or len(scores) != len(request['groups'])
            or any(type(value) not in (int, float) or not math.isfinite(value) for value in scores)):
        raise ValueError('invalid_scores')
    response = dict(schema_version=1, model_revision=request['model_revision'],
                    scores=[[g['id'], value] for g, value in zip(request['groups'], scores)])
    raw = json.dumps(response, ensure_ascii=False, allow_nan=False).encode('utf8')
    if len(raw) > RESPONSE_BYTES:
        raise ValueError('response_byte_limit')
    return raw
