import { useEffect, useMemo, useState } from "react"

import {
  extractDocument,
  getDocument,
  listDocuments,
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

function sliceUtf8(text: string, startByte: number, endByte: number) {
  const bytes = utf8Encoder.encode(text)
  const safeStart = Math.max(0, Math.min(startByte, bytes.length))
  const safeEnd = Math.max(safeStart, Math.min(endByte, bytes.length))
  return {
    before: utf8Decoder.decode(bytes.slice(0, safeStart)),
    match: utf8Decoder.decode(bytes.slice(safeStart, safeEnd)),
    after: utf8Decoder.decode(bytes.slice(safeEnd)),
  }
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

  const [documents, setDocuments] = useState<DocumentSummary[]>([])
  const [selectedDoc, setSelectedDoc] = useState<DocumentDetail | null>(null)
  const [selectedCandidate, setSelectedCandidate] =
    useState<StoredCandidate | null>(null)
  const [reviewResult, setReviewResult] = useState<ReviewResponse | null>(null)

  async function refreshList() {
    try {
      const rows = await listDocuments()
      setDocuments(rows)
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load documents")
    }
  }

  useEffect(() => {
    let cancelled = false
    listDocuments()
      .then((rows) => {
        if (!cancelled) {
          setDocuments(rows)
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setError(
            err instanceof Error ? err.message : "Failed to load documents",
          )
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
    setReviewResult(null)
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
      await refreshList()
    } catch (err) {
      setError(err instanceof Error ? err.message : "Extraction failed")
    } finally {
      setSubmitting(false)
    }
  }

  async function handleSelectDoc(id: string) {
    setError(null)
    setReviewResult(null)
    try {
      const detail = await getDocument(id)
      setSelectedDoc(detail)
      setSelectedCandidate(detail.candidates[0] ?? null)
    } catch (err) {
      setError(err instanceof Error ? err.message : "Failed to load document")
    }
  }

  async function handleVerifyCandidate(candidate: StoredCandidate) {
    if (!selectedDoc) return
    setError(null)
    try {
      const result = await runReview({
        question: "Verify selected evidence quote",
        selections: [
          {
            document_version_id: selectedDoc.id,
            quote: candidate.quote,
            start_byte: candidate.start_byte,
          },
        ],
      })
      setReviewResult(result)
    } catch (err) {
      setError(err instanceof Error ? err.message : "Review check failed")
    }
  }

  const highlighted = useMemo(() => {
    if (!selectedDoc || !selectedCandidate) return null
    return sliceUtf8(
      selectedDoc.normalized_text,
      selectedCandidate.start_byte,
      selectedCandidate.end_byte,
    )
  }, [selectedDoc, selectedCandidate])

  return (
    <div className="space-y-8">
      <div className="space-y-2">
        <h1 className="text-3xl font-semibold tracking-tight">
          Evidence Extraction &amp; Review
        </h1>
        <p className="text-muted-foreground">
          Submit saved UTF-8 HTML or text snapshots to inspect screening
          dispositions, normalized text, and UTF-8 byte-offset evidence spans.
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
              Parses saved input locally without network fetches.
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
                    <option value="html">HTML (screened + pruned)</option>
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
              <div className="mt-4 rounded-md border bg-muted/40 p-3 text-xs space-y-1">
                <div>
                  <span className="font-semibold">Disposition:</span>{" "}
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
              Select a version to inspect its normalized text and candidates.
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
          <CardContent className="grid gap-6 lg:grid-cols-2">
            <div className="space-y-2">
              <h2 className="text-sm font-semibold">Normalized Text</h2>
              <pre className="max-h-96 overflow-auto whitespace-pre-wrap rounded-md border bg-muted/30 p-3 text-xs leading-relaxed">
                {highlighted ? (
                  <>
                    {highlighted.before}
                    <mark className="rounded bg-yellow-300/80 px-0.5 text-foreground">
                      {highlighted.match}
                    </mark>
                    {highlighted.after}
                  </>
                ) : (
                  selectedDoc.normalized_text
                )}
              </pre>
            </div>

            <div className="space-y-3">
              <h2 className="text-sm font-semibold">
                Candidates ({selectedDoc.candidates.length})
              </h2>
              <div className="max-h-96 space-y-2 overflow-y-auto">
                {selectedDoc.candidates.map((cand) => (
                  <div
                    className={`rounded-md border p-3 text-xs space-y-1.5 ${
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
                    <div className="flex gap-2 pt-1">
                      <Button
                        onClick={() => setSelectedCandidate(cand)}
                        size="sm"
                        type="button"
                        variant="outline"
                      >
                        Highlight span
                      </Button>
                      <Button
                        onClick={() => void handleVerifyCandidate(cand)}
                        size="sm"
                        type="button"
                        variant="secondary"
                      >
                        Verify in review
                      </Button>
                    </div>
                  </div>
                ))}
              </div>
              {reviewResult ? (
                <div className="rounded-md border bg-muted/40 p-3 text-xs">
                  Verified {reviewResult.total_verified} selection(s):{" "}
                  <code>{JSON.stringify(reviewResult.tallies)}</code>
                </div>
              ) : null}
            </div>
          </CardContent>
        </Card>
      ) : null}
    </div>
  )
}
