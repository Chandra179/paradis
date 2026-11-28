import { useEffect, useMemo, useState } from "react"

import {
  extractDocument,
  getDocument,
  listAttempts,
  listDocuments,
  listReviews,
  runReview,
  type AttemptSummary,
  type DocumentDetail,
  type DocumentSummary,
  type ReviewResponse,
  type StoredCandidate,
} from "@/api/extraction"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

const utf8Encoder = new TextEncoder()
const utf8Decoder = new TextDecoder()

type TextSegment = {
  text: string
  role: "plain" | "match" | "subject" | "predicate" | "object"
}

type QueuedSelection = {
  key: string
  source_id: string
  document_version_id: string
  quote: string
  start_byte: number
}

function buildHighlightedSegments(
  text: string,
  candidate: StoredCandidate,
): TextSegment[] {
  const bytes = utf8Encoder.encode(text)
  const clamp = (pos: number) => Math.max(0, Math.min(pos, bytes.length))
  const decode = (start: number, end: number) =>
    utf8Decoder.decode(bytes.slice(start, end))

  const args = candidate.argument_evidence
  if (args) {
    const spans = [
      {
        start: clamp(args.subject.start_byte),
        end: clamp(args.subject.end_byte),
        role: "subject" as const,
      },
      {
        start: clamp(args.predicate.start_byte),
        end: clamp(args.predicate.end_byte),
        role: "predicate" as const,
      },
      {
        start: clamp(args.object.start_byte),
        end: clamp(args.object.end_byte),
        role: "object" as const,
      },
    ].sort((a, b) => a.start - b.start)

    const segments: TextSegment[] = []
    let cursor = 0
    for (const span of spans) {
      if (span.start > cursor) {
        segments.push({ text: decode(cursor, span.start), role: "plain" })
      }
      if (span.end > span.start) {
        segments.push({ text: decode(span.start, span.end), role: span.role })
      }
      cursor = Math.max(cursor, span.end)
    }
    if (cursor < bytes.length) {
      segments.push({ text: decode(cursor, bytes.length), role: "plain" })
    }
    return segments
  }

  const start = clamp(candidate.start_byte)
  const end = Math.max(start, clamp(candidate.end_byte))
  return [
    { text: decode(0, start), role: "plain" },
    { text: decode(start, end), role: "match" },
    { text: decode(end, bytes.length), role: "plain" },
  ]
}

const roleHighlightClass: Record<Exclude<TextSegment["role"], "plain">, string> = {
  match: "rounded bg-yellow-300/80 px-0.5 text-foreground",
  subject: "rounded bg-sky-200 px-0.5 text-foreground",
  predicate: "rounded bg-amber-200 px-0.5 text-foreground",
  object: "rounded bg-emerald-200 px-0.5 text-foreground",
}

const dispositionBadgeClass: Record<AttemptSummary["disposition"], string> = {
  accepted: "bg-emerald-100 text-emerald-900",
  skipped: "bg-slate-200 text-slate-800",
  uncertain: "bg-amber-100 text-amber-900",
}

export function DocumentsPage() {
  const [sourceId, setSourceId] = useState("sample-article")
  const [format, setFormat] = useState<"html" | "html_body" | "text">("html")
  const [content, setContent] = useState(
    "<main><p>Northwind Labs announced the new wing on 2026-11-27. The gallery is located in the west wing and holds 30 kg.</p></main>",
  )
  const [metadataOnly, setMetadataOnly] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [lastAttempt, setLastAttempt] = useState<AttemptSummary | null>(null)

  const [attempts, setAttempts] = useState<AttemptSummary[]>([])
  const [documents, setDocuments] = useState<DocumentSummary[]>([])
  const [selectedDoc, setSelectedDoc] = useState<DocumentDetail | null>(null)
  const [selectedCandidate, setSelectedCandidate] =
    useState<StoredCandidate | null>(null)

  const [reviewQuestion, setReviewQuestion] = useState(
    "Verify selected claims across sources",
  )
  const [reviewQueue, setReviewQueue] = useState<QueuedSelection[]>([])
  const [reviews, setReviews] = useState<ReviewResponse[]>([])
  const [reviewing, setReviewing] = useState(false)

  async function refreshAll() {
    try {
      const [docs, attemptRows, reviewRows] = await Promise.all([
        listDocuments(),
        listAttempts(),
        listReviews(),
      ])
      setDocuments(docs)
      setAttempts(attemptRows)
      setReviews(reviewRows)
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load data")
    }
  }

  useEffect(() => {
    let cancelled = false
    Promise.all([listDocuments(), listAttempts(), listReviews()])
      .then(([docs, attemptRows, reviewRows]) => {
        if (!cancelled) {
          setDocuments(docs)
          setAttempts(attemptRows)
          setReviews(reviewRows)
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Failed to load data")
        }
      })
    return () => {
      cancelled = true
    }
  }, [])

  async function handleExtract(event: React.FormEvent) {
    event.preventDefault()
    setSubmitting(true)
    setError(null)
    try {
      const response = await extractDocument({
        source_id: sourceId,
        format,
        content,
        metadata_only: metadataOnly,
      })
      setLastAttempt(response.attempt)
      if (response.document) {
        setSelectedDoc(response.document)
        setSelectedCandidate(response.document.candidates[0] ?? null)
      }
      await refreshAll()
    } catch (err) {
      setError(err instanceof Error ? err.message : "Extraction failed")
    } finally {
      setSubmitting(false)
    }
  }

  async function handleSelectDoc(id: string) {
    setError(null)
    try {
      const detail = await getDocument(id)
      setSelectedDoc(detail)
      setSelectedCandidate(detail.candidates[0] ?? null)
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load document")
    }
  }

  function handleToggleQueue(cand: StoredCandidate) {
    if (!selectedDoc) return
    const key = `${selectedDoc.id}:${cand.start_byte}:${cand.quote}`
    setReviewQueue((prev) =>
      prev.some((item) => item.key === key)
        ? prev.filter((item) => item.key !== key)
        : [
            ...prev,
            {
              key,
              source_id: selectedDoc.source_id,
              document_version_id: selectedDoc.id,
              quote: cand.quote,
              start_byte: cand.start_byte,
            },
          ],
    )
  }

  async function handleRunQueuedReview(event: React.FormEvent) {
    event.preventDefault()
    if (reviewQueue.length === 0) return
    setReviewing(true)
    setError(null)
    try {
      await runReview({
        question: reviewQuestion,
        selections: reviewQueue.map((item) => ({
          document_version_id: item.document_version_id,
          quote: item.quote,
          start_byte: item.start_byte,
        })),
      })
      setReviewQueue([])
      await refreshAll()
    } catch (err) {
      setError(err instanceof Error ? err.message : "Review failed")
    } finally {
      setReviewing(false)
    }
  }

  const highlightedSegments = useMemo(() => {
    if (!selectedDoc || !selectedCandidate) return null
    return buildHighlightedSegments(selectedDoc.normalized_text, selectedCandidate)
  }, [selectedDoc, selectedCandidate])

  return (
    <div className="space-y-8">
      <div className="space-y-2">
        <h1 className="text-3xl font-semibold tracking-tight">
          Evidence Extraction &amp; Review
        </h1>
        <p className="text-muted-foreground">
          Submit saved UTF-8 HTML or text snapshots to inspect screening
          dispositions, publisher metadata, UTF-8 byte-offset spans, and
          cross-source review tallies.
        </p>
      </div>

      {error ? (
        <div className="rounded-md border border-destructive/50 bg-destructive/10 px-4 py-3 text-sm text-destructive">
          {error}
        </div>
      ) : null}

      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>Submit Snapshot</CardTitle>
            <CardDescription>
              Parses saved input locally without outbound network fetches.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="space-y-4" onSubmit={handleExtract}>
              <div className="grid gap-4 sm:grid-cols-2">
                <label className="space-y-1 text-sm">
                  <span className="font-medium">Source ID</span>
                  <input
                    className="w-full rounded-md border bg-background px-3 py-2 text-sm"
                    onChange={(e) => setSourceId(e.target.value)}
                    required
                    value={sourceId}
                  />
                </label>
                <label className="space-y-1 text-sm">
                  <span className="font-medium">Format</span>
                  <select
                    className="w-full rounded-md border bg-background px-3 py-2 text-sm"
                    onChange={(e) =>
                      setFormat(e.target.value as "html" | "html_body" | "text")
                    }
                    value={format}
                  >
                    <option value="html">HTML (screened + legible)</option>
                    <option value="html_body">HTML body (unpruned)</option>
                    <option value="text">Plain text</option>
                  </select>
                </label>
              </div>

              <label className="block space-y-1 text-sm">
                <span className="font-medium">Snapshot Content</span>
                <textarea
                  className="min-h-36 w-full rounded-md border bg-background p-3 font-mono text-xs"
                  onChange={(e) => setContent(e.target.value)}
                  required
                  value={content}
                />
              </label>

              <label className="flex items-center gap-2 text-sm">
                <input
                  checked={metadataOnly}
                  onChange={(e) => setMetadataOnly(e.target.checked)}
                  type="checkbox"
                />
                <span>Metadata-only retention (withhold text and quotes)</span>
              </label>

              <Button disabled={submitting} type="submit">
                {submitting ? "Extracting..." : "Extract & Store"}
              </Button>
            </form>

            {lastAttempt ? (
              <div className="mt-4 space-y-1 rounded-md border bg-muted/40 p-3 text-xs">
                <div>
                  <span className="font-semibold">Last Attempt:</span>{" "}
                  {lastAttempt.disposition}
                  {lastAttempt.reason ? ` (${lastAttempt.reason})` : ""}
                </div>
                {lastAttempt.page_screening ? (
                  <div>
                    <span className="font-semibold">Screening signals:</span>{" "}
                    {lastAttempt.page_screening.signals.join(", ") || "none"}
                  </div>
                ) : null}
              </div>
            ) : null}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Stored Document Versions ({documents.length})</CardTitle>
            <CardDescription>
              Select a version to inspect normalized text, metadata, and candidates.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-2">
            {documents.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No retained documents yet.
              </p>
            ) : (
              <div className="max-h-80 space-y-2 overflow-y-auto">
                {documents.map((doc) => (
                  <button
                    className={`w-full rounded-md border p-3 text-left text-sm transition-colors hover:bg-accent ${
                      selectedDoc?.id === doc.id ? "border-primary bg-accent" : ""
                    }`}
                    key={doc.id}
                    onClick={() => void handleSelectDoc(doc.id)}
                    type="button"
                  >
                    <div className="flex items-center justify-between font-medium">
                      <span>{doc.source_id}</span>
                      <span className="text-xs text-muted-foreground">
                        {doc.candidate_count} candidates
                      </span>
                    </div>
                    <div className="mt-1 truncate font-mono text-xs text-muted-foreground">
                      {doc.normalizer_version} · {doc.id.slice(0, 16)}…
                    </div>
                  </button>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {selectedDoc ? (
        <Card>
          <CardHeader>
            <CardTitle>{selectedDoc.source_id}</CardTitle>
            <CardDescription className="font-mono text-xs">
              version {selectedDoc.id} · {selectedDoc.normalizer_version}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-6">
            {selectedDoc.metadata ? (
              <div className="rounded-md border bg-muted/30 p-3 text-xs space-y-1">
                <div className="font-semibold">Publisher Metadata</div>
                <div className="flex flex-wrap gap-x-6 gap-y-1 text-muted-foreground">
                  {selectedDoc.metadata.title ? (
                    <span>
                      <strong className="text-foreground">Title:</strong>{" "}
                      {selectedDoc.metadata.title} ({selectedDoc.metadata.title_origin})
                    </span>
                  ) : null}
                  {selectedDoc.metadata.published_at ? (
                    <span>
                      <strong className="text-foreground">Published:</strong>{" "}
                      {selectedDoc.metadata.published_at} (
                      {selectedDoc.metadata.published_origin})
                    </span>
                  ) : null}
                  {selectedDoc.metadata.authors?.length ? (
                    <span>
                      <strong className="text-foreground">Authors:</strong>{" "}
                      {selectedDoc.metadata.authors.join(", ")}
                    </span>
                  ) : null}
                </div>
                {selectedDoc.metadata.disagreements?.length ? (
                  <div className="pt-1 text-amber-800">
                    <strong>Metadata disagreements:</strong>{" "}
                    {selectedDoc.metadata.disagreements
                      .map(
                        (d) =>
                          `${d.field}: ${d.primary_origin}="${d.primary_value}" vs ${d.secondary_origin}="${d.secondary_value}"`,
                      )
                      .join("; ")}
                  </div>
                ) : null}
              </div>
            ) : null}

            <div className="grid gap-6 lg:grid-cols-2">
              <div className="space-y-2">
                <div className="flex items-center justify-between">
                  <h2 className="text-sm font-semibold">Normalized Text</h2>
                  {selectedCandidate?.argument_evidence ? (
                    <div className="flex gap-2 text-[11px]">
                      <span className="rounded bg-sky-200 px-1.5 py-0.5">
                        Subject
                      </span>
                      <span className="rounded bg-amber-200 px-1.5 py-0.5">
                        Predicate
                      </span>
                      <span className="rounded bg-emerald-200 px-1.5 py-0.5">
                        Object
                      </span>
                    </div>
                  ) : null}
                </div>
                <pre className="max-h-96 overflow-auto whitespace-pre-wrap rounded-md border bg-muted/30 p-3 text-xs leading-relaxed">
                  {highlightedSegments
                    ? highlightedSegments.map((seg, idx) =>
                        seg.role === "plain" ? (
                          <span key={idx}>{seg.text}</span>
                        ) : (
                          <mark className={roleHighlightClass[seg.role]} key={idx}>
                            {seg.text}
                          </mark>
                        ),
                      )
                    : selectedDoc.normalized_text}
                </pre>
              </div>

              <div className="space-y-3">
                <h2 className="text-sm font-semibold">
                  Candidates ({selectedDoc.candidates.length})
                </h2>
                <div className="max-h-96 space-y-2 overflow-y-auto">
                  {selectedDoc.candidates.map((cand) => {
                    const queueKey = `${selectedDoc.id}:${cand.start_byte}:${cand.quote}`
                    const isQueued = reviewQueue.some((item) => item.key === queueKey)
                    return (
                      <div
                        className={`space-y-1.5 rounded-md border p-3 text-xs ${
                          selectedCandidate?.id === cand.id
                            ? "border-primary bg-accent/50"
                            : ""
                        }`}
                        key={cand.id}
                      >
                        <div className="flex items-center justify-between">
                          <span className="font-semibold uppercase tracking-wide">
                            {cand.kind} · {cand.rule_id}
                          </span>
                          <span className="font-mono text-muted-foreground">
                            [{cand.start_byte}..{cand.end_byte}]
                          </span>
                        </div>
                        <div className="font-mono text-muted-foreground">
                          {JSON.stringify(cand.value)}
                        </div>
                        <div className="flex flex-wrap gap-2 pt-1">
                          <Button
                            onClick={() => setSelectedCandidate(cand)}
                            size="sm"
                            type="button"
                            variant="outline"
                          >
                            Highlight span
                          </Button>
                          <Button
                            onClick={() => handleToggleQueue(cand)}
                            size="sm"
                            type="button"
                            variant={isQueued ? "default" : "secondary"}
                          >
                            {isQueued ? "Queued for review" : "Add to review"}
                          </Button>
                        </div>
                      </div>
                    )
                  })}
                </div>
              </div>
            </div>
          </CardContent>
        </Card>
      ) : null}

      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>
              Claim Review &amp; Corroboration ({reviewQueue.length} queued)
            </CardTitle>
            <CardDescription>
              Queue quotes across one or more documents to verify byte spans and
              tally unique source support.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <form className="space-y-3" onSubmit={handleRunQueuedReview}>
              <label className="block space-y-1 text-sm">
                <span className="font-medium">Review Question</span>
                <input
                  className="w-full rounded-md border bg-background px-3 py-2 text-sm"
                  onChange={(e) => setReviewQuestion(e.target.value)}
                  required
                  value={reviewQuestion}
                />
              </label>

              {reviewQueue.length === 0 ? (
                <p className="text-xs text-muted-foreground">
                  Select candidates above with &ldquo;Add to review&rdquo; to
                  verify them together.
                </p>
              ) : (
                <div className="max-h-40 space-y-1.5 overflow-y-auto">
                  {reviewQueue.map((item) => (
                    <div
                      className="flex items-center justify-between rounded border px-2.5 py-1.5 text-xs"
                      key={item.key}
                    >
                      <span className="truncate">
                        <strong>{item.source_id}:</strong> &ldquo;{item.quote}&rdquo;
                      </span>
                      <button
                        className="ml-2 text-muted-foreground hover:text-foreground"
                        onClick={() =>
                          setReviewQueue((prev) =>
                            prev.filter((q) => q.key !== item.key),
                          )
                        }
                        type="button"
                      >
                        Remove
                      </button>
                    </div>
                  ))}
                </div>
              )}

              <Button
                disabled={reviewing || reviewQueue.length === 0}
                type="submit"
              >
                {reviewing ? "Verifying..." : "Verify & Save Review"}
              </Button>
            </form>

            {reviews.length > 0 ? (
              <div className="space-y-2 border-t pt-4">
                <h3 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Saved Reviews ({reviews.length})
                </h3>
                <div className="max-h-56 space-y-2 overflow-y-auto">
                  {reviews.map((rev) => (
                    <div className="rounded-md border p-2.5 text-xs space-y-1" key={rev.id}>
                      <div className="flex items-center justify-between font-medium">
                        <span>{rev.question}</span>
                        <span className="text-muted-foreground">
                          {rev.total_verified} verified
                        </span>
                      </div>
                      <div className="font-mono text-muted-foreground">
                        tallies: {JSON.stringify(rev.tallies)}
                      </div>
                      {rev.unresolved.length > 0 ? (
                        <div className="text-destructive">
                          unresolved: {rev.unresolved.join("; ")}
                        </div>
                      ) : null}
                    </div>
                  ))}
                </div>
              </div>
            ) : null}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Acquisition Attempts ({attempts.length})</CardTitle>
            <CardDescription>
              Audit log of accepted, skipped, uncertain, and metadata-only inputs.
            </CardDescription>
          </CardHeader>
          <CardContent>
            {attempts.length === 0 ? (
              <p className="text-sm text-muted-foreground">
                No acquisition attempts recorded yet.
              </p>
            ) : (
              <div className="max-h-96 space-y-2 overflow-y-auto">
                {attempts.map((attempt) => (
                  <div
                    className="rounded-md border p-3 text-xs space-y-1"
                    key={attempt.id}
                  >
                    <div className="flex items-center justify-between">
                      <span className="font-medium">
                        {attempt.source_id} ({attempt.format})
                      </span>
                      <span
                        className={`rounded px-2 py-0.5 font-medium ${dispositionBadgeClass[attempt.disposition]}`}
                      >
                        {attempt.disposition}
                        {attempt.reason ? `: ${attempt.reason}` : ""}
                      </span>
                    </div>
                    {attempt.page_screening?.signals.length ? (
                      <div className="text-muted-foreground">
                        signals: {attempt.page_screening.signals.join(", ")}
                      </div>
                    ) : null}
                    {attempt.document_version_id ? (
                      <button
                        className="font-mono text-primary hover:underline"
                        onClick={() =>
                          void handleSelectDoc(attempt.document_version_id!)
                        }
                        type="button"
                      >
                        Inspect version {attempt.document_version_id.slice(0, 16)}…
                      </button>
                    ) : null}
                  </div>
                ))}
              </div>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
