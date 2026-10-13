// node apps/desktop/ui/plainmath.test.cjs  (run by scripts/gates.sh and CI)
const path = require("path");
const plainMath = require("./plainmath.js");
const cases = require(path.join(__dirname, "../../../contracts/plainmath-cases.json"));
let bad = 0;
for (const [input, want] of cases) {
  const got = plainMath(input);
  if (got !== want) {
    bad++;
    console.log(`FAIL ${JSON.stringify(input)}\n  got  ${JSON.stringify(got)}\n  want ${JSON.stringify(want)}`);
  }
}
console.log(bad ? `plainmath: ${bad} of ${cases.length} failed` : `plainmath: ${cases.length} cases ok`);
process.exit(bad ? 1 : 0);
