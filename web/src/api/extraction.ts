import { apiRequest } from "./client"

export type EvidenceSpan = {
  document_version_id: string
  start_byte: number
  end_byte: number
  quote: string
}

export type ArgumentEvidence = {
  subject: EvidenceSpan
  predicate: EvidenceSpan
  object: EvidenceSpan
}

export type MetadataDisagreement = {
  field: string
  primary_origin: string
  primary_value: string
  secondary_origin: string
  secondary_value: string
}

export type DocumentMetadata = {
  title?: string
  title_origin?: string
  published_at?: string
  published_origin?: string
  modified_at?: string
  modified_origin?: string
  authors?: string[]
  author_origin?: string
  disagreements?: MetadataDisagreement[]
}

export type StoredCandidate = {
  id: string
  extractor_version: string
  rule_id: string
  kind: "date" | "quantity" | "relationship"
  value: Record<string, string>
  start_byte: number
  end_byte: number
  quote: string
  argument_evidence?: ArgumentEvidence | null
}

export type DocumentSummary = {
  id: string
  source_id: string
  normalizer_version: string
  normalized_text_sha256: string
  created_at: string
  candidate_count: number
}

export type DocumentDetail = {
  id: string
  source_id: string
  normalizer_version: string
  payload_sha256: string
  normalized_text_sha256: string
  normalized_text: string
  created_at: string
  metadata?: DocumentMetadata | null
  candidates: StoredCandidate[]
}

export type AttemptSummary = {
  id: string
  source_id: string
  format: string
  disposition: "accepted" | "skipped" | "uncertain"
  reason: string | null
  document_version_id: string | null
  page_screening: { version: string; signals: string[] } | null
  metadata: DocumentMetadata | null
  metadata_only: boolean
  created_at: string
}

export type ExtractResponse = {
  attempt: AttemptSummary
  document: DocumentDetail | null
}

export type ReviewSelection = {
  document_version_id: string
  quote: string
  start_byte?: number
}

export type ReviewResponse = {
  id: string
  question: string
  total_verified: number
  tallies: Record<string, number>
  verified: Array<{
    document_version_id: string
    source_id: string
    quote: string
    start_byte: number
    end_byte: number
  }>
  unresolved: string[]
  created_at: string
}

export function extractDocument(payload: {
  source_id: string
  format: "html" | "html_body" | "text"
  content: string
  metadata_only: boolean
}) {
  return apiRequest<ExtractResponse>("/documents/extract", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}

export function listAttempts() {
  return apiRequest<AttemptSummary[]>("/attempts")
}

export function listDocuments() {
  return apiRequest<DocumentSummary[]>("/documents")
}

export function getDocument(id: string) {
  return apiRequest<DocumentDetail>(`/documents/${encodeURIComponent(id)}`)
}

export function listReviews() {
  return apiRequest<ReviewResponse[]>("/reviews")
}

export function runReview(payload: {
  question: string
  selections: ReviewSelection[]
}) {
  return apiRequest<ReviewResponse>("/reviews", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}
