import { invoke } from '@tauri-apps/api/core';
import { readFile, stat } from '@tauri-apps/plugin-fs';

import {
  FEEDBACK_LIMITS,
  type FeedbackAcceptedFileType,
  type FeedbackSubmissionRequest,
  type FeedbackSubmissionResult,
  type StagedFeedbackAttachment,
} from '@/lib/models/feedback/feedback';
import { feedbackFileNameFromPath } from '@/lib/utils/feedback/feedback';
import { InstallationIdentityService } from '@/lib/services/http/installation-identity-service';
import { LoggerService } from '@/lib/services/diagnostics/logger-service';

export type DroppedAttachmentIssue = 'tooLarge' | 'unavailable';

/** Preserves a stable issue code across the native-drop and presentation boundary. */
export class DroppedAttachmentError extends Error {
  constructor(readonly issue: DroppedAttachmentIssue) {
    super('feedback_dropped_attachment_' + issue);
    this.name = 'DroppedAttachmentError';
  }
}

/** Owns the native feedback draft and submission protocol. */
export class FeedbackService {
  /**
   * Reads only a path granted by Tauri's native drop scope. Checking metadata
   * before reading avoids copying an oversized file into the WebView merely to
   * discover that the feedback attachment limit has already been exceeded.
   */
  static async readDroppedAttachment(path: string, mimeType: FeedbackAcceptedFileType): Promise<File> {
    const metadata = await stat(path);
    if (!metadata.isFile) throw new DroppedAttachmentError('unavailable');
    if (metadata.size > FEEDBACK_LIMITS.attachmentBytes) throw new DroppedAttachmentError('tooLarge');
    const bytes = await readFile(path);
    return new File([bytes], feedbackFileNameFromPath(path), { type: mimeType });
  }

  static async stageAttachment(file: File, mimeType: FeedbackAcceptedFileType): Promise<StagedFeedbackAttachment> {
    const fileName = FeedbackService.encodeHeaderValue(file.name);
    return invoke<StagedFeedbackAttachment>('stage_feedback_attachment', await file.arrayBuffer(), {
      headers: {
        'x-mangodisk-file-name': fileName,
        'x-mangodisk-mime-type': mimeType,
      },
    });
  }

  static discardAttachments(tokens: string[]): Promise<void> {
    if (tokens.length === 0) return Promise.resolve();
    return invoke<void>('discard_feedback_attachments', { tokens });
  }

  static async submit(request: FeedbackSubmissionRequest): Promise<FeedbackSubmissionResult> {
    let installId: string | null = null;
    try {
      installId = await InstallationIdentityService.getOrCreateInstallId();
    } catch (error) {
      // Identity persistence must not prevent users from reporting a problem.
      LoggerService.warn('feedback', 'feedback_installation_identity_unavailable', {
        requestId: request.requestId,
        error,
      });
    }
    return invoke<FeedbackSubmissionResult>('submit_feedback', { request: { ...request, installId } });
  }

  private static encodeHeaderValue(value: string): string {
    const bytes = new TextEncoder().encode(value);
    let binary = '';
    for (const byte of bytes) binary += String.fromCharCode(byte);
    return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
  }
}
