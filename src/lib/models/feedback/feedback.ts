export const FEEDBACK_SCHEMA_VERSION = 1 as const;
export const FEEDBACK_LIMITS = {
  contentMinLength: 10,
  contentMaxLength: 5000,
  emailMaxLength: 254,
  attachmentCount: 5,
  attachmentBytes: 10 * 1024 * 1024,
} as const;

export const FEEDBACK_CATEGORY_IDS = {
  issue: 'issue',
  suggestion: 'suggestion',
  other: 'other',
} as const;

export type FeedbackCategory = (typeof FEEDBACK_CATEGORY_IDS)[keyof typeof FEEDBACK_CATEGORY_IDS];

export interface StagedFeedbackAttachment {
  token: string;
  displayName: string;
  mimeType: string;
  size: number;
}

export interface FeedbackSubmissionRequest {
  requestId: string;
  installId?: string | null;
  category: FeedbackCategory;
  content: string;
  email: string | null;
  locale: string;
  includeLogs: boolean;
  attachmentTokens: string[];
}

export interface FeedbackSubmissionResult {
  id: string;
  createdAt: string;
  submittedLogCount: number;
}

export const FEEDBACK_ATTACHMENT_FORMATS = [
  { mimeType: 'image/png', extensions: ['png'] },
  { mimeType: 'image/jpeg', extensions: ['jpg', 'jpeg'] },
  { mimeType: 'image/webp', extensions: ['webp'] },
  { mimeType: 'image/gif', extensions: ['gif'] },
  { mimeType: 'application/pdf', extensions: ['pdf'] },
  { mimeType: 'application/zip', extensions: ['zip'] },
  { mimeType: 'text/plain', extensions: ['txt', 'log'] },
  { mimeType: 'text/markdown', extensions: ['md', 'markdown'] },
  { mimeType: 'text/csv', extensions: ['csv'] },
  { mimeType: 'application/json', extensions: ['json'] },
  { mimeType: 'application/yaml', extensions: ['yaml', 'yml'] },
  { mimeType: 'application/toml', extensions: ['toml'] },
  { mimeType: 'application/rtf', extensions: ['rtf'] },
  { mimeType: 'application/msword', extensions: ['doc'] },
  { mimeType: 'application/vnd.ms-excel', extensions: ['xls'] },
  { mimeType: 'application/vnd.ms-powerpoint', extensions: ['ppt'] },
  { mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', extensions: ['docx'] },
  { mimeType: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet', extensions: ['xlsx'] },
  { mimeType: 'application/vnd.openxmlformats-officedocument.presentationml.presentation', extensions: ['pptx'] },
] as const;

export type FeedbackAcceptedFileType = (typeof FEEDBACK_ATTACHMENT_FORMATS)[number]['mimeType'];

export const FEEDBACK_ATTACHMENT_ACCEPT = FEEDBACK_ATTACHMENT_FORMATS.flatMap(format => [
  format.mimeType,
  ...format.extensions.map(extension => `.${extension}`),
]).join(',');
