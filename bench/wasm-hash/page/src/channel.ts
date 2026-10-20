import type { JobReply, JobRequest } from "./protocol";

type Answer = Exclude<JobReply, { type: "error" }>;

/** One worker and one request at a time. A worker that fails rejects the request it was on. */
export class Channel {
  private readonly worker = new Worker(new URL("./worker.ts", import.meta.url), {
    type: "module",
  });
  private pending: { resolve(answer: Answer): void; reject(error: Error): void } | null = null;

  constructor() {
    this.worker.onmessage = (event: MessageEvent<JobReply>) => {
      const reply = event.data;
      if (reply.type === "error") {
        this.fail(reply.message);
      } else {
        this.pending?.resolve(reply);
        this.pending = null;
      }
    };
    this.worker.onerror = (event) => this.fail(event.message || "the worker could not start");
    this.worker.onmessageerror = () =>
      this.fail("the worker sent a message the page could not read");
  }

  ask<T extends Answer["type"]>(
    request: JobRequest,
    type: T,
  ): Promise<Extract<Answer, { type: T }>> {
    return new Promise((resolve, reject) => {
      this.pending = {
        resolve: (answer) => {
          if (answer.type === type) {
            resolve(answer as Extract<Answer, { type: T }>);
          } else {
            reject(new Error(`the worker answered ${answer.type} to a request for ${type}`));
          }
        },
        reject,
      };
      this.worker.postMessage(request);
    });
  }

  close(): void {
    this.worker.terminate();
  }

  private fail(message: string): void {
    this.pending?.reject(new Error(message));
    this.pending = null;
  }
}
