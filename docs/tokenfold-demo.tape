Output docs/assets/tokenfold-demo.gif

# Window geometry optimized for GitHub README width (prevents browser blur & empty void)
Set Shell "bash"
Set FontSize 18
Set FontFamily "JetBrains Mono"
Set Width 900
Set Height 400
Set Padding 20
Set LineHeight 1.35
Set BorderRadius 8
Set Theme "Dracula"
Set WindowBar "Colorful"

# Keep the three CLI steps and their concise results on-screen.
Set FontSize 14
Set TypingSpeed 20ms
Hide
Type "same_json(){ python3 -c 'import json,sys; sys.exit(json.load(open(sys.argv[1])) != json.load(open(sys.argv[2])))' $@ && printf 'verified\n'; }; export PATH=$PWD/target/release:$PATH; input=examples/api_response.json; export PS1='$ '; clear"
Enter
Show
Sleep 400ms

## 1. Inspect shows baseline tokens.
Type "tokenfold inspect $input --receipt-format text | grep original_tokens"
Sleep 200ms
Enter
Sleep 1.5s

## 2. Compress shows reduced tokens and percentage saved.
Type "tokenfold compress $input -o compact.json --receipt-format text 2>&1 | grep -E 'comp|pct'"
Sleep 200ms
Enter
Sleep 2s

## 3. Decode confirms lossless verification.
Type "tokenfold decode compact.json -o response.json&&same_json $input response.json"
Sleep 200ms
Enter
Sleep 4s
