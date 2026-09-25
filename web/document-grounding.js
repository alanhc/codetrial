export const groundingStorageKey = "codetrial.interview-grounding.v1";
export const groundingConsentVersion = 1;
export const maxGroundingFileBytes = 64 * 1024;
// MAX_GROUNDING_TEXT_BYTES in src/agent.rs, and that is not a coincidence to
// be maintained by memory: the server drops over-budget grounding silently.
export const maxGroundingPacketBytes = 6 * 1024;

const encoder = new TextEncoder();
const limits = { requirements: 8, skills: 8, anchors: 6 };
const textLimit = 240;

export async function parseGroundingFile(file, kind) {
  if (!file) throw new Error("Choose a .txt file.");
  if (!/\.txt$/i.test(file.name || "") || file.type !== "text/plain") {
    throw new Error("Use a UTF-8 .txt file with text/plain type.");
  }
  if (file.size === 0) throw new Error("The file is empty.");
  if (file.size > maxGroundingFileBytes) throw new Error("The file must be 64 KiB or smaller.");
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(await file.arrayBuffer());
  } catch {
    throw new Error("The file is not valid UTF-8.");
  }
  const lines = normalizeLines(text);
  if (!lines.length) throw new Error("The file contains no usable text.");
  return kind === "jd" ? parseJd(lines) : parseResume(lines);
}

export function retainedSelection(selected, kind) {
  const replaced = kind === "jd" ? ["requirements"] : ["skills", "anchors"];
  const retained = { requirements: [], skills: [], anchors: [] };
  for (const group of Object.keys(retained)) {
    if (!replaced.includes(group)) retained[group] = [...(selected[group] || [])];
  }
  return retained;
}

export function selectedGroundingPacket(extracted, selected, consent) {
  const packet = {
    consentVersion: groundingConsentVersion,
    requirements: pick(extracted.requirements, selected.requirements).map(normalizeSnippet),
    skills: pick(extracted.skills, selected.skills).map(normalizeSnippet),
    anchors: pick(extracted.anchors, selected.anchors).map(normalizeSnippet),
  };
  const count = packet.requirements.length + packet.skills.length + packet.anchors.length;
  if (!count) return null;
  if (!consent) throw new Error("Agree to send only your selected snippets before starting.");

  // The server rejects a list holding the same snippet twice, and rejecting it
  // means dropping every snippet in the packet, not just the repeat. `pick`
  // only rules out choosing one index twice, so two lines that read alike in
  // the document still arrive as a pair. Said here, because the alternative is
  // an interview that quietly runs with no grounding at all.
  for (const field of ["requirements", "skills", "anchors"]) {
    if (new Set(packet[field]).size !== packet[field].length) {
      throw new Error("Two selected snippets are identical. Remove the repeat before starting.");
    }
  }

  // Counted over the same text the server counts, which is why the snippets
  // are normalized above rather than at the point of use: the server measures
  // what it stores, and measuring the raw selection here would be counting a
  // different string and calling it the same budget.
  const bytes = [...packet.requirements, ...packet.skills, ...packet.anchors]
    .reduce((total, text) => total + encoder.encode(text).length, 0);
  if (bytes > maxGroundingPacketBytes) {
    throw new Error("Selected snippets are too long. Select fewer or shorter snippets.");
  }
  return packet;
}

/// The normalization `grounding_array` applies in src/agent.rs, using the same
/// two Unicode properties it does: `char::is_control` is the Cc category, and
/// `split_whitespace` is the White_Space property. Spelling either as a
/// hand-written character class would agree with Rust today and drift at the
/// next edition of the tables.
function normalizeSnippet(text) {
  return String(text)
    .replace(/\p{Cc}/gu, " ")
    .split(/\p{White_Space}+/u)
    .filter(Boolean)
    .join(" ");
}

export function storeGroundingPacket(storage, packet) {
  if (!packet) {
    try { storage.removeItem(groundingStorageKey); } catch { /* no grounding must remain usable */ }
    return;
  }
  try {
    storage.setItem(groundingStorageKey, JSON.stringify(packet));
  } catch {
    throw new Error("Selected snippets could not be stored temporarily. Clear grounding to start normally.");
  }
}

export function consumeGroundingPacket(storage) {
  let raw = null;
  try {
    raw = storage.getItem(groundingStorageKey);
  } catch { return null; }
  try { storage.removeItem(groundingStorageKey); } catch { return null; }
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    return value?.consentVersion === groundingConsentVersion ? value : null;
  } catch {
    return null;
  }
}

function normalizeLines(text) {
  return text.replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, " ")
    .split(/\r?\n/).map((line) => line.replace(/\s+/g, " ").trim()).filter(Boolean);
}

// A bullet glyph is stripped on its own, because it is a list glyph and not
// part of any name, so it needs no space after it to say so. A run of dashes
// or asterisks stripped that freely would eat into real content, so those
// still only count as a marker once whitespace after them confirms it. A
// run, not one character: "-- " and "** " are what pasted markdown leaves.
//
// A numbered marker is stripped without a following space, because "1.Must"
// and "1.) Must" are common paste shapes and the "1." should not reach the
// interviewer. Its digits must be followed by "." or ")", which keeps "5G"
// and "3D" whole, and then by whitespace or two letters. A "." alone is not
// enough, since it follows a digit inside real tokens: "9.x Java" has one
// letter after it and stays whole, while "1.Must" and "1.Go" lose their
// marker. That is a heuristic, not a rule: "1.C experience" keeps its
// marker and "3.js experience" loses its "3.".
// \p{Nd} rather than \d, so a full-width digit is a marker digit like an
// ASCII one.
//
// Chinese lists are numbered "1、", "1．", "（1）" or "一、", and bulleted with
// "●", "■", "◆", "▪" or "・" as often as with "•", so those are markers too.
// "、" is the list comma and never ends a token, which is what lets it count
// as a marker's punctuation the way "." and ")" do; the same two-letter
// lookahead then keeps it off a bare number such as "3、5 years".
function clean(line) {
  return line.trimStart()
    .replace(/^(?:(?:-+|\*+)\s+|\p{Nd}+[.)．、）]+(?=\s|\p{L}{2})\s*|[(（]\p{Nd}+[)）]\s*|[一二三四五六七八九十]+、\s*|[•●■◆▪・]\s*)+/u, "")
    .slice(0, textLimit).trim();
}

// A token with no letter is digits and symbols only, wearing a list item's
// clothes, and that is not a skill on its own.
//
// A letter is what makes a token legible as a named thing: "ISO 27001" and
// "IEEE 754" keep the org name that scopes their number, "5G" and "3D" carry
// their own label, and a plain "5" or "27001" or "2015" carries no such
// scope, whether it arrived alone -- "Skills: 2025" -- or split off a shared
// prefix by parseResume's "," / ";" / "|" split -- "Skills: ISO 27001,
// 124141, 2015". Either way, there is nothing left to tell whether it is
// still part of a standard, a separate one, or an unrelated year. Rather
// than guess, every letterless token is dropped, with no exception: "24/7"
// and "100%" are the same digit-plus-symbol shape as "-50", "1-2", and
// "2020-2024", and none of them carry a letter to claim a meaning others
// would have to guess at.
//
// That costs bare part numbers -- 6502, 8051, 68000 -- which are real skills
// on a systems resume and have no letter either. The loss is accepted, not
// overlooked: "8051" and "2025" are the same shape, four bare digits, and
// nothing in the token says which is a part number and which is a year, so
// a filter that goes by shape and keeps one keeps the other. Both are
// dropped. A part number that names itself, "MOS 6502" or "Z80", is kept.
function unique(values, max) {
  return [...new Set(values.map(clean).filter((value) => /\p{L}/u.test(value)))].slice(0, max);
}

// The Chinese words a JD requirement and a resume anchor are written with,
// traditional and simplified. Matched without \b, which needs a word
// character on one side and so never fires between two Han characters: a
// Chinese JD used to yield no requirements at all, because every one of the
// English words it was matched against is spelled some other way there.
const cjkRequirement = /(要求|需求|必備|必备|必須|必须|具備|具备|熟悉|熟練|熟练|精通|經驗|经验|能力|了解|瞭解|理解|掌握|擅長|擅长|優先|优先|尤佳|加分|資格|资格|以上)/u;
const cjkAnchor = /(專案|项目|負責|负责|主導|主导|帶領|带领|開發|开发|實作|實現|实现|建立|建置|打造|設計|设计|改善|改進|改进|優化|优化|提升|降低|減少|减少|導入|导入)/u;

// A section heading such as "條件要求", "加分條件" or "專案經驗" is made of
// the very words the lines under it are matched on, so without this each
// heading would take one of the few snippets as a requirement or an anchor
// that says nothing. Named rather than guessed from length: "兩年以上經驗" is
// as short as a heading and is a real requirement.
const cjkHeading = /^(工作|職務|职务|職位|职位|崗位|岗位|任職|任职|應徵|应聘|其他|加分|優先|优先|專案|项目|專業|专业)?(條件|条件|要求|需求|資格|资格|項目|项目|項|项|經驗|经验|經歷|经历|技能|專長|专长|內容|内容|職責|职责)(要求|需求)?[:：]?$/u;

function parseJd(lines) {
  const marked = lines.filter((line) => /\b(required?|requirements?|must|should|experience|proficien|knowledge|ability)\b/i.test(line)
    || (cjkRequirement.test(line) && !cjkHeading.test(clean(line))));
  return { requirements: unique(marked, limits.requirements), skills: [], anchors: [] };
}

// A Chinese skills line is headed "技能：" with a full-width colon and lists
// its items with "、", "，" or "；" as often as with their ASCII forms.
function parseResume(lines) {
  const skillLines = lines.filter((line) => /^(skills?|technologies|stack|技能|專業技能|专业技能|專長|专长|技術|技术|技術棧|技术栈)\s*[:：]/i.test(line));
  const skills = skillLines.flatMap((line) => line.replace(/^[^:：]+[:：]/, "").split(/[,;|、，；｜]/));
  const anchors = lines.filter((line) => /\b(project|experience|built|led|created|implemented|delivered|improved|reduced|increased|developed)\b/i.test(line)
    || (cjkAnchor.test(line) && !cjkHeading.test(clean(line))));
  return { requirements: [], skills: unique(skills, limits.skills), anchors: unique(anchors, limits.anchors) };
}

function pick(values = [], indexes = []) {
  return [...new Set(indexes)].filter((index) => Number.isInteger(index) && index >= 0 && index < values.length)
    .map((index) => values[index]);
}
