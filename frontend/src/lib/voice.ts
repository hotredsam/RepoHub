// Thin wrapper around the Web Speech API (SpeechRecognition) for voice-to-text.
// Falls back gracefully (isSupported() === false) on browsers without support.

// The Web Speech API types are not in the standard lib DOM typings, so we
// declare the minimal surface we use.
interface SpeechRecognitionResultLike {
  0: { transcript: string };
  isFinal: boolean;
}
interface SpeechRecognitionEventLike {
  results: ArrayLike<SpeechRecognitionResultLike>;
  resultIndex: number;
}
interface SpeechRecognitionLike {
  lang: string;
  continuous: boolean;
  interimResults: boolean;
  start(): void;
  stop(): void;
  onresult: ((e: SpeechRecognitionEventLike) => void) | null;
  onerror: ((e: { error: string }) => void) | null;
  onend: (() => void) | null;
}

type SpeechRecognitionCtor = new () => SpeechRecognitionLike;

function getCtor(): SpeechRecognitionCtor | null {
  const w = window as unknown as {
    SpeechRecognition?: SpeechRecognitionCtor;
    webkitSpeechRecognition?: SpeechRecognitionCtor;
  };
  return w.SpeechRecognition ?? w.webkitSpeechRecognition ?? null;
}

export function isSupported(): boolean {
  return getCtor() !== null;
}

export interface VoiceHandlers {
  // Called with the latest transcript (final or interim).
  onResult: (text: string, isFinal: boolean) => void;
  onError?: (msg: string) => void;
  onEnd?: () => void;
}

export class VoiceInput {
  private rec: SpeechRecognitionLike | null = null;
  private handlers: VoiceHandlers;

  constructor(handlers: VoiceHandlers) {
    this.handlers = handlers;
  }

  start(lang = "en-US"): boolean {
    const Ctor = getCtor();
    if (!Ctor) {
      this.handlers.onError?.("Voice input is not supported in this browser.");
      return false;
    }
    const rec = new Ctor();
    rec.lang = lang;
    rec.continuous = true;
    rec.interimResults = true;
    rec.onresult = (e) => {
      let text = "";
      let isFinal = false;
      for (let i = e.resultIndex; i < e.results.length; i++) {
        const r = e.results[i];
        text += r[0].transcript;
        if (r.isFinal) isFinal = true;
      }
      this.handlers.onResult(text, isFinal);
    };
    rec.onerror = (e) => this.handlers.onError?.(e.error);
    rec.onend = () => this.handlers.onEnd?.();
    rec.start();
    this.rec = rec;
    return true;
  }

  stop(): void {
    this.rec?.stop();
    this.rec = null;
  }
}
