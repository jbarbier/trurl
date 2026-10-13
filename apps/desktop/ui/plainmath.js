// Math written by models in LaTeX ($\frac{1}{2}$, \boxed{42}), shown as plain
// text a learner can read: 1/2, 42. No math renderer to download or pin; the
// terminal app does the same in apps/tui/src/text.rs, and both are checked
// against contracts/plainmath-cases.json.
(function (root) {
  "use strict";

  const SYMBOLS = {
    times: "×", cdot: "·", div: "÷", pm: "±", mp: "∓",
    le: "≤", leq: "≤", ge: "≥", geq: "≥", neq: "≠", ne: "≠", approx: "≈", equiv: "≡",
    infty: "∞", pi: "π", alpha: "α", beta: "β", gamma: "γ", delta: "δ", theta: "θ",
    lambda: "λ", mu: "μ", sigma: "σ", Sigma: "Σ", Delta: "Δ", omega: "ω",
    to: "→", rightarrow: "→", Rightarrow: "⇒", implies: "⇒", leftarrow: "←", iff: "⇔",
    ldots: "…", dots: "…", cdots: "…", degree: "°", circ: "°", percent: "%",
    sum: "Σ", prod: "Π", in: "∈", cup: "∪", cap: "∩", forall: "∀", exists: "∃",
    quad: " ", qquad: "  ", ",": " ", ";": " ", "!": "", " ": " ",
  };
  const SUP = { 0: "⁰", 1: "¹", 2: "²", 3: "³", 4: "⁴", 5: "⁵", 6: "⁶", 7: "⁷", 8: "⁸", 9: "⁹", "+": "⁺", "-": "⁻", n: "ⁿ" };

  // The content of the {group} starting at s[i] === "{", and the index after it.
  function group(s, i) {
    let depth = 0;
    for (let j = i; j < s.length; j++) {
      if (s[j] === "{") depth++;
      else if (s[j] === "}" && --depth === 0) return [s.slice(i + 1, j), j + 1];
    }
    return [s.slice(i + 1), s.length];
  }

  function needsParens(x) {
    return /[+\-×÷·/ ]/.test(x.trim()) && !/^\(.*\)$/.test(x.trim());
  }

  function sup(x) {
    const chars = [...x];
    return chars.every((c) => SUP[c]) ? chars.map((c) => SUP[c]).join("") : "^(" + x + ")";
  }

  // Convert one piece of LaTeX math to text.
  function convert(src) {
    let out = "";
    let i = 0;
    while (i < src.length) {
      const c = src[i];
      if (c === "\\") {
        const m = /^\\([A-Za-z]+|.)/.exec(src.slice(i));
        const name = m ? m[1] : "";
        i += m ? m[0].length : 1;
        // Commands that take a {group} ignore the spaces before it.
        if (["frac", "dfrac", "tfrac", "sqrt", "boxed", "text", "textbf", "mathrm", "mathbf", "operatorname", "mbox", "textit", "mathit", "left", "right"].includes(name))
          while (src[i] === " ") i++;
        if (name === "frac" || name === "dfrac" || name === "tfrac") {
          let a = "", b = "";
          if (src[i] === "{") [a, i] = group(src, i);
          if (src[i] === "{") [b, i] = group(src, i);
          a = convert(a);
          b = convert(b);
          out += (needsParens(a) ? "(" + a + ")" : a) + "/" + (needsParens(b) ? "(" + b + ")" : b);
        } else if (name === "sqrt") {
          let a = "";
          if (src[i] === "{") [a, i] = group(src, i);
          a = convert(a);
          out += "√" + (needsParens(a) ? "(" + a + ")" : a);
        } else if (["boxed", "text", "textbf", "mathrm", "mathbf", "operatorname", "mbox", "textit", "mathit"].includes(name)) {
          let a = "";
          if (src[i] === "{") [a, i] = group(src, i);
          out += name === "text" || name === "mbox" ? a : convert(a);
        } else if (name === "left" || name === "right" || name === "big" || name === "Big" || name === "displaystyle") {
          // \left( and \right): just the bracket.
        } else if (name === "{" || name === "}" || name === "%" || name === "$" || name === "#" || name === "&" || name === "_") {
          out += name;
        } else if (name in SYMBOLS) {
          out += SYMBOLS[name];
        } else {
          out += name;
        }
      } else if (c === "^") {
        i++;
        let a;
        if (src[i] === "{") [a, i] = group(src, i);
        else a = src[i++] || "";
        out += sup(convert(a));
      } else if (c === "_") {
        i++;
        let a;
        if (src[i] === "{") [a, i] = group(src, i);
        else a = src[i++] || "";
        out += convert(a);
      } else if (c === "{" || c === "}") {
        i++;
      } else if (c === "&") {
        i++;
      } else {
        out += c;
        i++;
      }
    }
    return out
      .replace(/([([{])\s+/g, "$1")
      .replace(/\s+([)\]}])/g, "$1")
      .replace(/ {2,}/g, " ")
      .trim();
  }

  // Find the math in a model's answer and convert it. A lone "$5" or "$5 and
  // $10" is money, not math: inline $…$ must not start or end with a space.
  function prose(text) {
    return text
      .replace(/\$\$([\s\S]+?)\$\$/g, (_, m) => convert(m))
      .replace(/\\\[([\s\S]+?)\\\]/g, (_, m) => convert(m))
      .replace(/\\\(([\s\S]+?)\\\)/g, (_, m) => convert(m))
      .replace(/(^|[^\\$\w])\$(?=\S)([^$\n]*?\S)\$(?!\d)/g, (_, pre, m) => pre + convert(m))
      // The few commands models also write outside math.
      .replace(/\\(?:boxed|text|textbf)\{[^{}]*\}/g, (m) => convert(m))
      .replace(/\\q?quad\b ?/g, " ");
  }

  // Code is left exactly as written: `$HOME` in a shell example is not math.
  function plainMath(text) {
    if (!text || !/[$\\]/.test(text)) return text || "";
    return text
      .split(/(```[\s\S]*?(?:```|$)|`[^`\n]*`)/)
      .map((part, i) => (i % 2 ? part : prose(part)))
      .join("");
  }

  if (typeof module !== "undefined" && module.exports) module.exports = plainMath;
  else root.plainMath = plainMath;
})(this);
