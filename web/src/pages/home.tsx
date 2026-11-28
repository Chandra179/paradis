import { ArrowRight, Boxes, Code2, Server } from "lucide-react"
import { Link } from "react-router-dom"

import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

const foundations = [
  {
    icon: Code2,
    title: "React + Vite",
    description: "Fast local inspection of normalized text, metadata, and evidence spans.",
  },
  {
    icon: Boxes,
    title: "Evidence Provenance",
    description: "Every candidate and review tally anchors to UTF-8 byte offsets.",
  },
  {
    icon: Server,
    title: "Rust API",
    description: "Snapshot screening, extraction, and SQLite persistence via paradis-server.",
  },
]

export function HomePage() {
  return (
    <div className="space-y-10">
      <section className="max-w-3xl space-y-5">
        <p className="text-sm font-medium text-primary">Paradis workbench</p>
        <h1 className="text-4xl font-semibold tracking-tight sm:text-6xl">
          Evidence-backed extraction &amp; claim review.
        </h1>
        <p className="max-w-2xl text-lg text-muted-foreground">
          Submit HTML or text snapshots, inspect screening dispositions and publisher
          metadata, and verify corroborating quotes against versioned documents.
        </p>
        <Button asChild>
          <Link to="/documents">
            Open Documents Workbench <ArrowRight className="size-4" />
          </Link>
        </Button>
      </section>

      <section className="grid gap-4 md:grid-cols-3" aria-label="Frontend foundation">
        {foundations.map((foundation) => {
          const Icon = foundation.icon

          return (
            <Card key={foundation.title}>
              <CardHeader>
                <div className="mb-2 flex size-10 items-center justify-center rounded-lg bg-primary/10 text-primary">
                  <Icon className="size-5" />
                </div>
                <CardTitle>{foundation.title}</CardTitle>
                <CardDescription>{foundation.description}</CardDescription>
              </CardHeader>
              <CardContent>
                <div className="h-1 w-16 rounded-full bg-primary/20" />
              </CardContent>
            </Card>
          )
        })}
      </section>
    </div>
  )
}
