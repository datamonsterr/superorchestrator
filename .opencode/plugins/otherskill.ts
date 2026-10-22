// OtherSkill managed adapter
// otherskill:portable
import {homedir} from "node:os";
/** OpenCode 1.x adapter. The installed module exports its hooks directly per the native API. */
interface Message {
  info: { role: string; id: string; sessionID: string };
  parts: Array<{
    type: string;
    text?: string;
    synthetic?: boolean;
    id?: string;
    sessionID?: string;
    messageID?: string;
  }>;
}
function createOpenCodePlugin(options: {
  command: string[];
  cwd: string;
  timeoutMs?: number;
  run?: (argv: string[]) => Promise<string>;
}) {
  const cache = new Map<string, Promise<string>>();
  const run =
    options.run ??
    (async (argv: string[]) => {
      const process = Bun.spawn(argv, {
        cwd: options.cwd,
        stdin: "ignore",
        stdout: "pipe",
        stderr: "pipe",
        signal: AbortSignal.timeout(options.timeoutMs ?? 60000),
      });
      const [output, errors] = await Promise.all([
        new Response(process.stdout).text(),
        new Response(process.stderr).text(),
      ]);
      if (errors)
        console.error("[otherskill] routing diagnostics available via doctor");
      if ((await process.exited) !== 0) throw new Error("find failed");
      return output;
    });
  return {
    "experimental.chat.messages.transform": async (
      _input: unknown,
      output: { messages: Message[] },
    ) => {
      const user = [...output.messages]
        .reverse()
        .find((m) => m.info.role === "user");
      if (!user) return;
      const id = `otherskill-${user.info.id}`;
      for (const message of output.messages)
        message.parts = message.parts.filter(
          (part) => !(part.synthetic && part.id?.startsWith("otherskill-")),
        );
      const prompt = user.parts
        .filter(
          (p) =>
            p.type === "text" && !p.synthetic && typeof p.text === "string",
        )
        .map((p) => p.text)
        .join("\n");
      if (!prompt.trim()) return;
      const key = `${user.info.sessionID}:${user.info.id}:${prompt}`;
      try {
        let pending = cache.get(key);
        if (!pending) {
          pending = run([...options.command, "--prompt", prompt]);
          cache.set(key, pending);
          if (cache.size > 128) cache.delete(cache.keys().next().value!);
        }
        const result: unknown = JSON.parse(await pending);
        if (
          !result ||
          typeof result !== "object" ||
          !("context" in result) ||
          typeof result.context !== "string"
        )
          throw new Error("invalid find output");
        if (result.context && !user.parts.some((part) => part.id === id))
          user.parts.push({
            type: "text",
            text: result.context,
            synthetic: true,
            id,
            sessionID: user.info.sessionID,
            messageID: user.info.id,
          });
      } catch {
        console.error(
          "[otherskill] OpenCode routing unavailable; continuing without context",
        );
      }
    },
  };
}

export const OtherSkill = async ({directory}:{directory:string}) => createOpenCodePlugin({command:[homedir()+"/.local/share/otherskill/bin/otherskill",...["find","--agent","opencode","--model","inception/mercury-decide:free"]],cwd:directory,timeoutMs:106250});
