/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_FIXTRACE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
