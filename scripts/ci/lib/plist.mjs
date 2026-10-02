// A small XML property list reader: enough for the Info.plist that `tauri bundle` writes (dict, array,
// string, true/false, integer, real, date, data).
const decode = (text) => {
  return text
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", '"')
    .replaceAll("&apos;", "'")
    .replaceAll("&amp;", "&");
};

/**
 * Parses an XML plist into plain values (dict → object, array → array, true/false → boolean).
 */
export const parsePlist = (xml) => {
  const tokens = [
    ...xml.matchAll(/<[?!][^>]*>|<(\/?)(\w+)[^>]*?(\/?)>|([^<]+)/g),
  ]
    .filter((match) => {
      return !match[0].startsWith("<?") && !match[0].startsWith("<!");
    })
    .map((match) => {
      if (match[4] !== undefined) {
        return { text: decode(match[4]) };
      }

      return {
        close: match[1] === "/",
        empty: match[3] === "/",
        name: match[2],
      };
    })
    .filter((token) => {
      return token.text === undefined || token.text.trim() !== "";
    });
  let position = 0;

  const textUntil = (name) => {
    let text = "";
    while (!(tokens[position].close && tokens[position].name === name)) {
      text += tokens[position].text ?? "";
      position += 1;
    }
    position += 1;
    return text;
  };

  const value = () => {
    const token = tokens[position];
    position += 1;
    if (token.name === "true" || token.name === "false") {
      if (!token.empty) {
        position += 1;
      }
      return token.name === "true";
    }
    if (token.empty) {
      return token.name === "dict" ? {} : token.name === "array" ? [] : "";
    }
    if (token.name === "dict") {
      const result = {};
      while (!(tokens[position].close && tokens[position].name === "dict")) {
        position += 1;
        const key = textUntil("key");
        result[key] = value();
      }
      position += 1;
      return result;
    }
    if (token.name === "array") {
      const result = [];
      while (!(tokens[position].close && tokens[position].name === "array")) {
        result.push(value());
      }
      position += 1;
      return result;
    }
    if (["string", "date", "data"].includes(token.name)) {
      return textUntil(token.name);
    }
    if (token.name === "integer" || token.name === "real") {
      return Number(textUntil(token.name));
    }

    throw new Error(`unsupported plist element <${token.name}>`);
  };

  while (tokens[position] && tokens[position].name !== "plist") {
    position += 1;
  }
  position += 1;

  return value();
};
