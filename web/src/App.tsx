import { BrowserRouter, Route, Routes } from "react-router-dom"

import { AppShell } from "@/components/app-shell"
import { DocumentsPage } from "@/pages/documents"
import { HomePage } from "@/pages/home"

export default function App() {
  return (
    <BrowserRouter>
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<HomePage />} />
          <Route path="documents" element={<DocumentsPage />} />
        </Route>
      </Routes>
    </BrowserRouter>
  )
}
