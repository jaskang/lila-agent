export type GitFileStatus = "modified" | "added" | "deleted" | "renamed" | "copied" | "untracked" | "unmerged";

export type GitFileChange = {
  path: string;
  from?: string;
  status: GitFileStatus;
  staged: boolean;
  unstaged: boolean;
  added?: number;
  removed?: number;
};

export type GitStatus = {
  isRepo: boolean;
  branch?: string;
  ahead?: number;
  behind?: number;
  files: GitFileChange[];
};

export type GitFileDiff = {
  path: string;
  patch: string;
  binary?: boolean;
  truncated?: boolean;
};

export type ProjectFileContent = {
  /** Path relative to project root when inside the project, otherwise absolute path. */
  path: string;
  /** Real absolute path on disk (used for reveal / open in external app). */
  fullPath: string;
  size: number;
  isBinary: boolean;
  isImage: boolean;
  truncated: boolean;
  content: string;
};
