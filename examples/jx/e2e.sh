#!/bin/sh
# Checks of a built jx: examples/jx/e2e.sh path/to/jx
# (mito build examples/jx/jx.mito -o jx)
JX=${1:-./jx}
pass=0; fail=0
J='{"name":"ann","tags":["a","b"],"users":[{"n":"x","age":31,"ok":true},{"n":"y","age":17,"ok":false}],"o":{},"s":"hé"}'

# expect NAME EXPECTED_OUTPUT EXPECTED_EXIT -- ARGS... (the JSON is on stdin)
expect() {
    name=$1; want=$2; code=$3; shift 3
    got=$(printf '%s' "$J" | "$JX" "$@" 2>&1); rc=$?
    if [ "$got" = "$want" ] && [ "$rc" = "$code" ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL $name: got ($rc) [$got], want ($code) [$want]"; fi
}

expect identity-compact '{"name":"ann","tags":["a","b"],"users":[{"n":"x","age":31,"ok":true},{"n":"y","age":17,"ok":false}],"o":{},"s":"hé"}' 0 -c .
expect field '"ann"' 0 .name
expect raw 'ann' 0 -r .name
expect element '"b"' 0 '.tags[1]'
expect nested '"x"' 0 '.users[0].n'
expect missing-field 'null' 0 .nothing
expect missing-element 'null' 0 '.tags[9]'
expect quoted-key '"ann"' 0 '."name"'
expect each '"x"
"y"' 0 '.users[].n'
expect select-number '"x"' 0 '.users[] | select(.age >= 18) | .n'
expect select-less '"y"' 0 '.users[] | select(.age < 18) | .n'
expect select-string '31' 0 '.users[] | select(.n == "x") | .age'
expect select-not-equal '"y"' 0 '.users[] | select(.n != "x") | .n'
expect select-truthy '"x"' 0 '.users[] | select(.ok) | .n'
expect length-array '2' 0 '.users | length'
expect length-string '2' 0 '.s | length'
expect length-object '5' 0 '. | length'
expect type-object '"object"' 0 '.users[0] | type'
expect type-number '"number"' 0 '.users[0].age | type'
expect empty-object '{}' 0 -c .o
expect pipe-spaces '"b"' 0 '  .tags  |  .[1]  '
expect object-values '"x"
31
true' 0 '.users[0] | .[]'
expect empty-object-values '' 0 '.o | .[]'
expect error-iterate 'jx: cannot iterate over this value' 1 '.name | .[]'
expect error-step 'jx: unknown step `frobnicate`' 1 '.name | frobnicate'
expect error-char 'jx: unexpected `)` in the filter' 1 '.name )'
expect error-last 'jx: length and type make a new value: they must be the last step' 1 '.tags | length | .'
expect error-no-filter '{"error":"`filter` is required"}' 1

got=$(printf '{"a":' | "$JX" .a 2>&1); rc=$?
if [ "$rc" = 1 ] && [ "$got" = "jx: the input is not valid JSON" ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL truncated-input: ($rc) $got"; fi
got=$(printf '' | "$JX" . 2>&1); rc=$?
if [ "$rc" = 1 ] && [ "$got" = "jx: no JSON on stdin" ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL empty-input: ($rc) $got"; fi
big=$(awk 'BEGIN { printf "["; for (i = 0; i < 20000; i++) { if (i) printf ","; printf "{\"i\":%d,\"s\":\"item %d\"}", i, i } printf "]" }')
got=$(printf '%s' "$big" | "$JX" '.[] | select(.i >= 19998) | .s' | tr '\n' ' ')
if [ "$got" = '"item 19998" "item 19999" ' ]; then pass=$((pass+1)); else fail=$((fail+1)); echo "FAIL large-input: $got"; fi

echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
