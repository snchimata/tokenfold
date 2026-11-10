"""Protocol 2 direct-child resident worker. Protocol 1 snapshot remains unchanged."""
import contextlib
import json
import os
import struct
import sys
from .protocol import REQUEST_BYTES, RESPONSE_BYTES, decode, validate_request
from .integrity import validate_approval


def read_exact(stream,size):
    data=bytearray()
    while len(data)<size:
        chunk=stream.read(size-len(data))
        if not chunk: raise EOFError()
        data.extend(chunk)
    return bytes(data)


def read_frame(stream):
    size=struct.unpack('<I',read_exact(stream,4))[0]
    if not 0<size<=REQUEST_BYTES: raise ValueError('frame_limit')
    value=decode(read_exact(stream,size))
    if (not isinstance(value,dict) or set(value)!={'protocol_version','request_id','op','body'}
            or type(value['protocol_version']) is not int or value['protocol_version']!=2
            or type(value['request_id']) is not int or not 0<value['request_id']<=9007199254740991
            or value['op'] not in ('load','score','close')): raise ValueError('envelope')
    if value["op"]!="score" and size>4096: raise ValueError("control_frame_limit")
    return value


def write_frame(stream,request_id,op,body):
    raw=json.dumps(dict(protocol_version=2,request_id=request_id,op=op,body=body),ensure_ascii=False,allow_nan=False,separators=(',',':')).encode()
    if len(raw)>(RESPONSE_BYTES if op=='scores' else 4096): raise ValueError('response_frame_limit')
    stream.write(struct.pack('<I',len(raw))+raw);stream.flush()


def offline(event,_):
    if event in ('socket.connect','socket.getaddrinfo','socket.bind','subprocess.Popen','os.system'):
        raise ValueError('offline_direct_child_only')


def serve(input_stream,output_stream):
    from .ettin import EttinDevelopmentScorer
    scorer=None;previous=0;request_id=0
    while True:
        try:
            frame=read_frame(input_stream);request_id=frame['request_id']
            if request_id<=previous: raise ValueError('monotonic_id_required')
            previous=request_id;op=frame['op'];body=frame['body']
            if op=='close':
                if body!={}: raise ValueError('close_body')
                write_frame(output_stream,request_id,'closed',{});return 0
            if op=='load':
                if not isinstance(body,dict) or set(body)!={'config','warmup'} or type(body['warmup']) is not bool:
                    raise ValueError('load_body')
                config=validate_approval(body['config'])
                if scorer is None:
                    scorer=EttinDevelopmentScorer(config['bundle'],config['manifest_sha256'],development=True,device=config['device'],precision=config['precision'])
                elif config!=approved_config: raise ValueError('load_config_changed')
                approved_config=config
                if body['warmup'] and scorer._model is None: scorer._load()
                write_frame(output_stream,request_id,'ready',dict(phase='loaded' if body['warmup'] else 'initialized'))
            elif op=='score':
                if scorer is None: raise ValueError('load_required')
                request=validate_request(body,scorer.revision)
                try: windows,receipt=scorer.admit(request)
                except ValueError:
                    write_frame(output_stream,request_id,'error',dict(code='selection_limit'));return 1
                write_frame(output_stream,request_id,'ready',dict(phase='admitted'))
                if scorer._model is None: scorer._load()
                write_frame(output_stream,request_id,'ready',dict(phase='loaded'))
                response=scorer.score_admitted(request,windows,receipt)
                write_frame(output_stream,request_id,'scores',dict(response=response,diagnostics={k:receipt[k] for k in ('outside_training_group_counts','cross_window_context_unqualified')}))
        except EOFError: return 0
        except Exception:
            if request_id: write_frame(output_stream,request_id,'error',dict(code='failure'))
            return 1


def main():
    os.environ.update(HF_HUB_OFFLINE='1',TRANSFORMERS_OFFLINE='1',CUDA_VISIBLE_DEVICES='0',
        TOKENIZERS_PARALLELISM='false',USERNAME='tokenfold-worker')
    sys.addaudithook(offline)
    # Native libraries may print diagnostics; never mix them into transport or log input.
    with open(os.devnull,'w') as quiet,contextlib.redirect_stdout(quiet),contextlib.redirect_stderr(quiet):
        return serve(sys.stdin.buffer,sys.__stdout__.buffer)
