import json, os, pathlib, struct, sys, time
root=pathlib.Path(__file__).resolve().parents[2]
birth=0
if os.name=='nt':
    import ctypes
    created=ctypes.c_uint64();exited=ctypes.c_uint64();kernel=ctypes.c_uint64();user=ctypes.c_uint64()
    ctypes.windll.kernel32.GetProcessTimes(ctypes.c_void_p(-1),ctypes.byref(created),ctypes.byref(exited),ctypes.byref(kernel),ctypes.byref(user))
    birth=created.value
with (root/'pids').open('a') as f: f.write(str(os.getpid())+':'+str(birth)+'\n')
assert not any(k in os.environ for k in ('OPENAI_API_KEY','ANTHROPIC_API_KEY','PYTHONPATH','PATH','HTTP_PROXY','HTTPS_PROXY'))
def read(n):
    data=b''
    while len(data)<n:
        chunk=sys.stdin.buffer.read(n-len(data))
        if not chunk: raise EOFError
        data+=chunk
    return data

def send(id,op,body):
    raw=json.dumps(dict(protocol_version=2,request_id=id,op=op,body=body)).encode()
    sys.stdout.buffer.write(struct.pack('<I',len(raw))+raw);sys.stdout.buffer.flush()
while True:
    try: value=json.loads(read(struct.unpack('<I',read(4))[0]))
    except EOFError: break
    id=value['request_id'];op=value['op'];body=value['body']
    with (root/'trace').open('a') as f: f.write(str(id)+':'+op+':'+str(body.get('query',''))+'\n')
    mode=(root/'mode').read_text()
    if op=='load':
        if mode=='startup_timeout': time.sleep(60)
        send(id,'ready',dict(phase='loaded' if body['warmup'] else 'initialized'))
        if mode=='stopped_reader': time.sleep(60)
    elif op=='close':
        send(id,'closed',{})
        if mode=='slow_close': (root/'closing').write_text('closing');time.sleep(1)
        break
    else:
        query=body['query']
        if query=='limit': send(id,'error',dict(code='selection_limit'));break
        if query=='duplicate':
            raw=b'{"protocol_version":1,"protocol_version":2,"request_id":'+str(id).encode()+b',"op":"error","body":{"code":"selection_limit"}}'
            sys.stdout.buffer.write(struct.pack('<I',len(raw))+raw);sys.stdout.buffer.flush();time.sleep(60)
        if query=='oversize': sys.stdout.buffer.write(struct.pack('<I',65537));sys.stdout.buffer.flush();time.sleep(60)
        send(id,'ready',dict(phase='admitted'))
        if query=='cold_load': time.sleep(1.2)
        send(id,'ready',dict(phase='loaded'))
        (root/'active').write_text(query)
        if query=='hold': time.sleep(60)
        if query=='inference_timeout': time.sleep(2)
        pairs=[[g['id'],float('nan') if query=='nan' else 1.25] for g in body['groups']]
        if query=='id_mismatch': pairs[0][0]='wrong'
        response=dict(schema_version=1,model_revision='wrong' if query=='revision' else body['model_revision'],scores=pairs)
        send(id+1 if query=='stale' else id,'scores',dict(response=response,diagnostics=dict(outside_training_group_counts=True,cross_window_context_unqualified=False)))
