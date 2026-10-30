#!/usr/bin/env bash
# Offline verification: recordings and fake executables only. Never runs setup/person/teardown.
set -euo pipefail
A=$(cd -- "$(dirname -- "$0")" && pwd -P)
# shellcheck source=tests/acceptance/acc.sh
source "$A/acc.sh"
S=$(mktemp -d "${TMPDIR:-/tmp}/ga-fixture.XXXXXX")
S=$(cd -- "$S" && pwd -P)
trap 'rm -rf "$S"' EXIT # This script owns every file beneath its newly created directory.
mkdir -p "$S/project" "$S/bin" "$S/glen-bin"
# Evidence retains conversation lines and drops Claude's internal beta-header telemetry.
printf '%s\n' '{"type":"api-request","betas":["oauth-2025-04-20","claude-code-20250219"]}' '{"type":"user","message":{"content":"hello"}}' > "$S/transcript.jsonl"
copy_transcript "$S/transcript.jsonl" "$S/filtered-transcript.jsonl"
jq -se 'length==1 and .[0].type=="user" and .[0].message.content=="hello"' "$S/filtered-transcript.jsonl" >/dev/null
copy_transcript "$S/missing-transcript.jsonl" "$S/absent-transcript.jsonl"
[[ ! -e $S/absent-transcript.jsonl ]] || die 'missing transcript produced evidence'
# Large help output must finish writing before either required-command check exits early.
cat > "$S/help-glen" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ ${1-} == --help ]]
printf '  inbox ...\n  install-extension ...\n'
perl -e 'print "x" x 1024, "\n" for 1..200;'
EOF
chmod +x "$S/help-glen"
for iteration in {1..20}; do
    (require_w3_surface "$S/help-glen") || die "large help gate failed on iteration $iteration"
done
jq -n --arg s "$S" '{scratch:$s,project:($s+"/project"),R:{id:"11111111-1111-1111-1111-11111aaaaaaa",handle:"aaaaaaa"},L:{id:"22222222-2222-2222-2222-22222bbbbbbb",handle:"bbbbbbb"},X:{id:"33333333-3333-3333-3333-33333ccccccc",handle:"ccccccc"},NONCE_R:"R-nonce",NONCE_L:"L-nonce",NONCE_T:"T-nonce"}' > "$S/manifest.json"
jq -cn --slurpfile m "$S/manifest.json" '
  $m[0] as $m |
  [ {a:["ls"],out:{data:{sessions:[$m.R,$m.L,$m.X]}},exit:0},
    {a:["tail","aaaaaaa"],out:{data:{session:$m.R}},exit:0},
    {a:["send","aaaaaaa","hi"],out:{data:{job:{session_id:$m.R.id,route:"rpc"},assistant_text:$m.NONCE_R}},exit:0},
    {a:["send","bbbbbbb","hi"],out:{data:{job:{session_id:$m.L.id,route:"live"},replies:[{text:$m.NONCE_L}]}},exit:0},
    {a:["inbox"],out:{data:{messages:[{text:$m.NONCE_T}]}},exit:0},
    {a:["send","ccccccc","hi"],out:{error:{hint:"close that session"}},exit:6}
  ] | to_entries[] | .key as $i | .value as $v |
  {t:"start",id:($i|tostring),argv:$v.a,ts:($i*10)},
  {t:"end",id:($i|tostring),argv:$v.a,exit:$v.exit,ms:20,stdout:$v.out}' > "$S/calls.ndjson"
printf 'R-nonce L-nonce T-nonce close that session\n' > "$S/report.md"
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls")}}]}}' > "$S/transcript.jsonl"
# Telemetry tool-shaped data must not affect the verdict or appear in copied evidence.
printf '%s\n' '{"type":"api-request","betas":["oauth-2025-04-20","claude-code-20250219"],"message":{"content":[{"type":"tool_use","name":"Telemetry"}]}}' >> "$S/transcript.jsonl"
"$A/acc.sh" check b "$S"
cp "$S/calls.ndjson" "$S/good.ndjson"
cp "$S/transcript.jsonl" "$S/good-transcript.jsonl"
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls; echo \"exit=$?\"")}}]}}' > "$S/transcript.jsonl"
"$A/acc.sh" check b "$S"
jq -e '.pass and (.checks|any(.id=="AC-415-isolation" and .pass))' "$S/verdict.json" >/dev/null
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls; cat /etc/passwd")}}]}}' > "$S/transcript.jsonl"
if "$A/acc.sh" check b "$S"; then echo 'compound b recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-415-isolation" and (.pass|not)) and all(.[];.pass or .id=="AC-415-isolation")' "$S/verdict.json" >/dev/null
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
jq -cn --arg s "$S" '{type:"assistant",message:{content:[{type:"tool_use",id:"t1",name:"Bash",input:{command:($s+"/bin/glen ls | jq .")}}]}}' > "$S/transcript.jsonl"
if "$A/acc.sh" check b "$S"; then echo 'piped b recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-415-isolation" and (.pass|not)) and all(.[];.pass or .id=="AC-415-isolation")' "$S/verdict.json" >/dev/null
cp "$S/good-transcript.jsonl" "$S/transcript.jsonl"
jq -c 'if .t=="end" and .stdout.data.assistant_text? then .stdout.data.assistant_text="missing" else . end' "$S/good.ndjson" > "$S/calls.ndjson"
printf '{"t":"start","id":"killed","argv":["ls"],"ts":100}\n{"t":"guard_refused","argv":["send","foreign"]}\n' >> "$S/calls.ndjson"
if "$A/acc.sh" check b "$S"; then echo 'bad recording passed' >&2; exit 1; fi
jq -e 'all(["nonce_R","paired_calls","guard_refused"][]; . as $id | any($ARGS.named.v.checks[];.id==$id and (.pass|not)))' --argjson v "$(cat "$S/verdict.json")" <<< null >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
printf '%s\n' '{"type":"tool_use","id":"skill","name":"Skill","input":{"skill":"glen"}}' '{"type":"tool_result","tool_use_id":"skill","content":"Skill loaded"}' '{"type":"tool_use","id":"bash","name":"Bash","input":{"command":"glen ls"}}' '{"type":"tool_result","tool_use_id":"bash","content":"sessions listed"}' > "$S/b3.jsonl"
"$A/acc.sh" check b3 "$S"
jq -e '.pass and (.checks|all(.pass))' "$S/verdict.json" >/dev/null
cp "$S/b3.jsonl" "$S/b3-good.jsonl"
# A denied Skill loader must not count as a discovered skill load, even without is_error.
jq -c 'if .tool_use_id?=="skill" then .content="Permission denied" else . end' "$S/b3-good.jsonl" > "$S/b3.jsonl"
if "$A/acc.sh" check b3 "$S"; then echo 'denied Skill recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-420-loaded" and (.pass|not))' "$S/verdict.json" >/dev/null
jq -c 'if .tool_use_id?=="bash" then .content="Permission denied" else . end' "$S/b3-good.jsonl" > "$S/b3.jsonl"
if "$A/acc.sh" check b3 "$S"; then echo 'denied b3 recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-420-permission" and (.pass|not))' "$S/verdict.json" >/dev/null
jq -cn '{message:{content:[{type:"tool_use",id:"probe",name:"Bash",input:{command:"glen ls"}},
  {type:"tool_result",tool_use_id:"probe",content:({v:1,ok:true,data:{sessions:[]}}|tojson)}]}}' > "$S/b2-probe.jsonl"
printf '%s\n' '{"type":"tool_use","id":"control","name":"Bash","input":{"command":"glen ls"}}' '{"type":"tool_result","tool_use_id":"control","content":"Permission denied"}' > "$S/b2-control.jsonl"
"$A/acc.sh" check b2 "$S"
jq -e '.pass and (.checks|all(.pass))' "$S/verdict.json" >/dev/null
# Text content arrays use the same JSON parsing path.
jq -c '.message.content[1].content |= [{type:"text",text:.}]' "$S/b2-probe.jsonl" > "$S/probe-array.jsonl"
cp "$S/probe-array.jsonl" "$S/b2-probe.jsonl"
"$A/acc.sh" check b2 "$S"
jq -e '.pass and (.checks|any(.id=="AC-416-probe" and .pass))' "$S/verdict.json" >/dev/null
jq -cn '{message:{content:[{type:"tool_use",id:"probe",name:"Bash",input:{command:"glen ls"}},
  {type:"tool_result",tool_use_id:"probe",content:({v:1,ok:false,error:{message:"Permission denied"}}|tojson)}]}}' > "$S/b2-probe.jsonl"
if "$A/acc.sh" check b2 "$S"; then echo 'denied b2 recording passed' >&2; exit 1; fi
jq -e '(.pass|not) and (.checks|any(.id=="AC-416-probe" and (.pass|not)))' "$S/verdict.json" >/dev/null
# Usage rejections are not delivery attempts; sandboxed sends still count.
jq -cn --arg s "$S" '
  [ {a:["ls"],out:{data:{sessions:[]}},exit:0},
    {a:["tail","aaaaaaa"],out:{data:{}},exit:0},
    {a:["send","aaaaaaa","hi","--cwd",($s+"/project")],out:{error:{code:"usage"}},exit:2},
    {a:["send","aaaaaaa","hi"],out:{error:{hint:"sandbox hint",details:{reason:"sandboxed",network:false}}},exit:6},
    {a:["send","bbbbbbb","hi"],out:{error:{hint:"sandbox hint",details:{reason:"sandboxed",network:false}}},exit:6}
  ] | to_entries[] | .key as $i | .value as $v |
  {t:"start",id:($i|tostring),argv:$v.a,ts:($i*10)},
  {t:"end",id:($i|tostring),argv:$v.a,exit:$v.exit,ms:20,stdout:$v.out}' > "$S/calls.ndjson"
"$A/acc.sh" check c1 "$S" >/dev/null || true
jq -e '.checks|any(.id=="AC-417-sandbox" and .pass)' "$S/verdict.json" >/dev/null
cp "$S/calls.ndjson" "$S/c1-mixed.ndjson"
# Usage errors alone must not satisfy the nonempty delivery-attempt requirement.
jq -c 'select(.argv[0]!="send" or (.argv|index("--cwd")))' "$S/c1-mixed.ndjson" > "$S/calls.ndjson"
if "$A/acc.sh" check c1 "$S"; then echo 'usage-only c1 recording passed' >&2; exit 1; fi
jq -e '.checks|any(.id=="AC-417-sandbox" and (.pass|not))' "$S/verdict.json" >/dev/null
cp "$S/good.ndjson" "$S/calls.ndjson"
# Missing recordings must retain every mode's check IDs in a valid failing verdict.
for mode in a b b2 b3 c1 c2; do
    "$A/acc.sh" check "$mode" "$S" >/dev/null || true
    jq '[.checks[].id]' "$S/verdict.json" > "$S/check-ids.json"
    rm "$S/calls.ndjson"
    if "$A/acc.sh" check "$mode" "$S"; then echo "missing $mode recording passed" >&2; exit 1; fi
    jq -e --slurpfile ids "$S/check-ids.json" --arg path "$S/calls.ndjson" \
      '(.pass|not) and ([.checks[].id]==$ids[0]) and (.checks|length>0) and all(.checks[]; (.pass|not) and (.evidence|contains($path) and contains("missing recording")))' "$S/verdict.json" >/dev/null
    cp "$S/good.ndjson" "$S/calls.ndjson"
done
# Exercise the guard with a fake binary, including --as before target and escaped cwd.
printf '#!/usr/bin/env bash\nprintf '\''{"v":1,"ok":true,"data":{}}\\n'\''\n' > "$S/glen-bin/glen"
chmod +x "$S/glen-bin/glen"
printf '#!/usr/bin/env bash\nACC_S=%q\n' "$S" > "$S/bin/glen"
tail -n +2 "$A/glen-wrapper.sh" >> "$S/bin/glen"; chmod +x "$S/bin/glen"
# c2 help calls bypass the guard but retain normal paired recordings.
: > "$S/calls.ndjson"
for help in --help -h; do
    "$S/bin/glen" send "$help" >/dev/null
done
"$S/bin/glen" send --as --help >/dev/null
jq -se '. as $calls | length==6 and all(.[];.t!="guard_refused") and
  ([.[]|select(.t=="start")]|length)==3 and
  all(.[]|select(.t=="start"); . as $s | any($calls[];.t=="end" and .id==$s.id and .argv==$s.argv and .exit==0 and .stdout=={v:1,ok:true,data:{}}))' "$S/calls.ndjson" >/dev/null
"$A/acc.sh" check c2 "$S" >/dev/null || true
jq -e '.checks|any(.id=="guard_refused" and .pass)' "$S/verdict.json" >/dev/null
# Help after the separator is a target, and foreign handles remain guarded.
for target in --help -h deadbeef; do
    if "$S/bin/glen" send -- "$target" hi >/dev/null 2>&1; then
        echo 'unguarded c2 target allowed' >&2; exit 1
    else
        [[ $? == 99 ]] || { echo 'unexpected c2 guard exit' >&2; exit 1; }
    fi
done
jq -se '[.[]|select(.t=="guard_refused")]|length==3' "$S/calls.ndjson" >/dev/null
"$S/bin/glen" send --as auto aaaaaaa hi >/dev/null
"$S/bin/glen" send @new hi --cwd "$S/project" >/dev/null
if "$S/bin/glen" send @new hi --cwd "$S/project/../.." >/dev/null 2>&1; then echo 'escape allowed' >&2; exit 1; fi
if "$S/bin/glen" send deadbeef hi >/dev/null 2>&1; then echo 'foreign session allowed' >&2; exit 1; fi
if "$S/bin/glen" install-extension >/dev/null 2>&1; then echo 'extension install allowed' >&2; exit 1; fi
# Fake token is assembled at runtime so the repository does not itself contain a credential-shaped string.
mkdir "$S/leak"
printf 'token = "sk-ant-%s-%s"\n' api03 "$(printf 'a1B2c3D4e5F6g7H8i9J0%.0s' {1..5} | head -c 93)AA" > "$S/leak/fake.txt"
if gitleaks dir --no-banner --redact "$S/leak" >/dev/null 2>&1; then echo 'fake token was not detected' >&2; exit 1; fi
printf 'offline harness fixture checks passed\n'
