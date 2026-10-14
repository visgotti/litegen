import json, sys
txt = open(sys.argv[1]).read()
data = json.loads(txt[txt.index('{'):])
for f, r in data.items():
    print('=' * 10, f)
    if 'error' in r:
        print('  ERROR', r['error'][:400])
    for o in r.get('objs', []):
        extra = {k: v for k, v in o.items() if k not in ('type', 'name')}
        print('  ', o['type'], repr(o['name']), json.dumps(extra)[:700])
    for a in r.get('anims', []):
        print('  ANIM', a['name'], 'dur', a['duration'])
        for t in a['tracks']:
            print('     ', t[:220])
    for l in r.get('logs', []):
        print('  LOG', l[:220])
