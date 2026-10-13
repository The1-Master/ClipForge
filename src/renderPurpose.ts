import type { RenderJob } from './api';

/** Only explicit native markers identify a review preview; legacy quality/title is not evidence. */
export function isReviewPreview(job: RenderJob): boolean {
  return job.analysisClips?.ruleId === 'ai-preview' || !!job.sourceSnapshot?.highlights.some(highlight => Array.isArray(highlight.tags) && highlight.tags.includes('ai-preview'));
}
