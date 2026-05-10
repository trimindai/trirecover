const RELEASES_URL = "https://github.com/trimindai/trirecover/releases/latest";
const DIRECT_DOWNLOAD = "https://github.com/trimindai/trirecover/releases/latest/download/TriRecover_0.2.0_x64-setup.exe";
const REPO_URL = "https://github.com/trimindai/trirecover";

export default function Page() {
  return (
    <main className="min-h-screen">
      {/* Top bar */}
      <header className="border-b border-border bg-surface/60 backdrop-blur-sm sticky top-0 z-20">
        <div className="mx-auto max-w-6xl flex items-center justify-between px-6 py-4">
          <a href="#" className="flex items-center gap-3">
            <span className="grid h-9 w-9 place-items-center rounded-lg bg-gradient-to-br from-primary to-[#2d61c7] font-bold tracking-tight">
              TR
            </span>
            <span className="font-semibold">TriRecover</span>
          </a>
          <nav className="hidden items-center gap-6 text-sm text-muted md:flex">
            <a href="#features" className="hover:text-white">Features</a>
            <a href="#how" className="hover:text-white">How it works</a>
            <a href="#faq" className="hover:text-white">FAQ</a>
            <a href={REPO_URL} target="_blank" rel="noreferrer" className="hover:text-white">GitHub</a>
          </nav>
          <a
            href={RELEASES_URL}
            target="_blank"
            rel="noreferrer"
            className="rounded-lg bg-primary px-4 py-2 text-sm font-medium text-white transition hover:bg-primaryHover"
          >
            Download
          </a>
        </div>
      </header>

      {/* Hero */}
      <section className="relative overflow-hidden">
        <div className="absolute inset-0 grid-bg" aria-hidden />
        <div className="absolute inset-0 hero-glow" aria-hidden />
        <div className="relative mx-auto max-w-6xl px-6 py-24 md:py-32">
          <div className="mx-auto max-w-3xl text-center">
            <span className="inline-flex items-center gap-2 rounded-full border border-[#2c5436] bg-[#1f3a25] px-3 py-1 text-xs font-semibold tracking-wider text-success">
              <span className="h-1.5 w-1.5 rounded-full bg-success" />
              READ-ONLY · FORENSICALLY SOUND
            </span>
            <h1 className="mt-6 text-4xl font-bold leading-tight tracking-tight md:text-6xl">
              Recover what was lost.
              <br />
              <span className="bg-gradient-to-r from-primary to-success bg-clip-text text-transparent">
                Touch nothing on the way.
              </span>
            </h1>
            <p className="mt-6 text-lg text-muted md:text-xl">
              TriRecover is a professional file-carving recovery tool for Windows.
              The source disk image is opened read-only — your data is reconstructed
              without ever modifying the original.
            </p>
            <div className="mt-10 flex flex-col items-center justify-center gap-3 sm:flex-row">
              <a
                href={DIRECT_DOWNLOAD}
                className="group inline-flex items-center gap-3 rounded-xl bg-primary px-7 py-4 text-base font-semibold text-white shadow-lg shadow-primary/20 transition hover:bg-primaryHover"
              >
                <DownloadIcon />
                Download for Windows
                <span className="rounded-md bg-white/15 px-2 py-0.5 text-xs font-medium text-white/90">
                  v0.2.0
                </span>
              </a>
              <a
                href={REPO_URL}
                target="_blank"
                rel="noreferrer"
                className="inline-flex items-center gap-2 rounded-xl border border-border bg-surface2 px-6 py-4 text-base font-medium text-white/90 transition hover:bg-[#232938]"
              >
                <GithubIcon />
                Source on GitHub
              </a>
            </div>
            <p className="mt-4 text-sm text-muted">
              Free for personal use · GPL-3.0 or commercial license · Windows 10/11 · 64-bit
            </p>
          </div>

          {/* App preview */}
          <div className="relative mx-auto mt-16 max-w-5xl">
            <div className="absolute -inset-4 rounded-3xl bg-gradient-to-r from-primary/30 to-success/20 blur-2xl" aria-hidden />
            <div className="relative overflow-hidden rounded-2xl border border-border bg-surface shadow-2xl">
              <AppPreview />
            </div>
          </div>
        </div>
      </section>

      {/* Features */}
      <section id="features" className="border-t border-border bg-bg">
        <div className="mx-auto max-w-6xl px-6 py-24">
          <div className="mx-auto max-w-2xl text-center">
            <h2 className="text-3xl font-bold tracking-tight md:text-4xl">
              Forensics-grade. By default.
            </h2>
            <p className="mt-4 text-muted">
              Every choice in TriRecover protects the integrity of the source.
              Write paths simply do not exist.
            </p>
          </div>

          <div className="mt-16 grid gap-6 md:grid-cols-3">
            <FeatureCard
              icon={<ShieldIcon />}
              title="Read-only by design"
              body="The source image is opened with read-only handles. There are no write IOCTLs in the codebase — by construction, not by promise."
            />
            <FeatureCard
              icon={<SearchIcon />}
              title="Smart signature carving"
              body="20+ formats with per-format validators: JPEG marker walk, PNG chunk + CRC, MP4/MOV atom chains, Matroska EBML, OOXML refinement, and more."
            />
            <FeatureCard
              icon={<BoltIcon />}
              title="Built for big drives"
              body="Streaming chunked reads with adaptive validation windows up to 64 MiB. Handles multi-terabyte images without loading them into memory."
            />
            <FeatureCard
              icon={<FileIcon />}
              title="Pictures, video, docs"
              body="JPG, PNG, GIF, BMP, TIFF, MP4, MOV, MKV, AVI, PDF, DOCX, XLSX, PPTX, ZIP, RAR, 7z, PSD, AI, TXT, CSV, SQL."
            />
            <FeatureCard
              icon={<MeterIcon />}
              title="Recoverability score"
              body="Each candidate is graded 0–100 based on how complete its structure looks — header, footer, internal cross-references — so you fix the highest-confidence files first."
            />
            <FeatureCard
              icon={<CodeIcon />}
              title="Open and auditable"
              body="Rust workspace, dual-licensed GPL-3.0-or-later or commercial. Inspect every line that touches your data."
            />
          </div>
        </div>
      </section>

      {/* How it works */}
      <section id="how" className="border-t border-border bg-surface/30">
        <div className="mx-auto max-w-6xl px-6 py-24">
          <div className="mx-auto max-w-2xl text-center">
            <h2 className="text-3xl font-bold tracking-tight md:text-4xl">
              Three steps. No surprises.
            </h2>
          </div>
          <div className="mt-14 grid gap-8 md:grid-cols-3">
            <Step
              n="01"
              title="Image the drive"
              body="Use any read-only imager (or your existing forensic image — .img, .dd, .bin, .raw) and point TriRecover at the file."
            />
            <Step
              n="02"
              title="Scan"
              body="The carver streams through the image, identifies file boundaries by signature + structure, and reports candidates with live progress."
            />
            <Step
              n="03"
              title="Recover"
              body="Pick the files you want, choose an output folder, and TriRecover writes clean copies — never to the source."
            />
          </div>
        </div>
      </section>

      {/* FAQ */}
      <section id="faq" className="border-t border-border bg-bg">
        <div className="mx-auto max-w-3xl px-6 py-24">
          <h2 className="text-center text-3xl font-bold tracking-tight md:text-4xl">
            Frequently asked
          </h2>
          <div className="mt-12 space-y-4">
            <Faq q="Will TriRecover write to my disk?">
              No. The source image is opened read-only and there are no write
              syscalls against it anywhere in the code. Recovered files are
              written to a separate output folder you choose.
            </Faq>
            <Faq q="Does it work on a live drive, or do I need an image first?">
              v0.1 reads disk images (.img / .dd / .bin / .raw). Image the drive
              first using a write-blocker or a tool like dd / FTK Imager, then
              run TriRecover against the image.
            </Faq>
            <Faq q="Which Windows versions are supported?">
              Windows 10 and Windows 11, 64-bit.
            </Faq>
            <Faq q="What is the license?">
              Dual-licensed: GPL-3.0-or-later (free, with copyleft) or a
              commercial license for closed-source use. See the GitHub repo for
              details.
            </Faq>
            <Faq q="Is the installer signed?">
              Not yet. Windows SmartScreen may show a warning on first launch —
              click &ldquo;More info&rdquo; → &ldquo;Run anyway&rdquo;. Code-signing is on the roadmap.
            </Faq>
          </div>
        </div>
      </section>

      {/* Final CTA */}
      <section className="border-t border-border">
        <div className="mx-auto max-w-4xl px-6 py-20 text-center">
          <h2 className="text-3xl font-bold tracking-tight md:text-4xl">
            Get your files back.
          </h2>
          <p className="mt-4 text-muted">
            Free download. No account. No telemetry.
          </p>
          <div className="mt-8 flex flex-col items-center justify-center gap-3 sm:flex-row">
            <a
              href={DIRECT_DOWNLOAD}
              className="inline-flex items-center gap-3 rounded-xl bg-primary px-7 py-4 text-base font-semibold text-white shadow-lg shadow-primary/20 transition hover:bg-primaryHover"
            >
              <DownloadIcon />
              Download for Windows
            </a>
          </div>
        </div>
      </section>

      <footer className="border-t border-border">
        <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-3 px-6 py-8 text-sm text-muted md:flex-row">
          <div>© {new Date().getFullYear()} TriMind AI. All rights reserved.</div>
          <div className="flex items-center gap-4">
            <a href={REPO_URL} target="_blank" rel="noreferrer" className="hover:text-white">GitHub</a>
            <a href="https://trimind.tech" target="_blank" rel="noreferrer" className="hover:text-white">trimind.tech</a>
          </div>
        </div>
      </footer>
    </main>
  );
}

function FeatureCard({
  icon,
  title,
  body,
}: {
  icon: React.ReactNode;
  title: string;
  body: string;
}) {
  return (
    <div className="rounded-2xl border border-border bg-surface p-6 transition hover:border-primary/40 hover:bg-surface2">
      <div className="grid h-10 w-10 place-items-center rounded-lg bg-primary/15 text-primary">
        {icon}
      </div>
      <h3 className="mt-4 text-lg font-semibold">{title}</h3>
      <p className="mt-2 text-sm leading-relaxed text-muted">{body}</p>
    </div>
  );
}

function Step({ n, title, body }: { n: string; title: string; body: string }) {
  return (
    <div className="rounded-2xl border border-border bg-surface p-6">
      <div className="font-mono text-sm tracking-widest text-primary">{n}</div>
      <h3 className="mt-3 text-lg font-semibold">{title}</h3>
      <p className="mt-2 text-sm leading-relaxed text-muted">{body}</p>
    </div>
  );
}

function Faq({ q, children }: { q: string; children: React.ReactNode }) {
  return (
    <details className="group rounded-xl border border-border bg-surface px-5 py-4 open:bg-surface2">
      <summary className="flex cursor-pointer list-none items-center justify-between font-medium">
        {q}
        <span className="ml-4 text-muted transition group-open:rotate-45">+</span>
      </summary>
      <p className="mt-3 text-sm leading-relaxed text-muted">{children}</p>
    </details>
  );
}

/* ---------- App preview (inline mock of the Tauri UI) ---------- */
function AppPreview() {
  return (
    <div className="bg-bg">
      {/* fake titlebar */}
      <div className="flex items-center justify-between border-b border-border bg-surface px-4 py-2.5">
        <div className="flex items-center gap-2">
          <span className="h-3 w-3 rounded-full bg-[#ff6363]" />
          <span className="h-3 w-3 rounded-full bg-[#f5b740]" />
          <span className="h-3 w-3 rounded-full bg-[#46c46f]" />
        </div>
        <div className="text-xs text-muted">TriRecover v0.2.0</div>
        <div className="w-12" />
      </div>

      <div className="p-6">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <span className="grid h-9 w-9 place-items-center rounded-lg bg-gradient-to-br from-primary to-[#2d61c7] text-sm font-bold">
              TR
            </span>
            <div>
              <div className="text-sm font-semibold">TriRecover</div>
              <div className="text-[11px] text-muted">v0.2.0</div>
            </div>
          </div>
          <span className="rounded-full border border-[#2c5436] bg-[#1f3a25] px-2.5 py-0.5 text-[10px] font-bold tracking-widest text-success">
            READ-ONLY
          </span>
        </div>

        <div className="mt-5 rounded-xl border border-border bg-surface p-4">
          <div className="text-xs text-muted">3. Recover</div>
          <div className="mt-3 overflow-hidden rounded-lg border border-border">
            <table className="w-full text-left text-xs">
              <thead className="bg-surface2 text-muted">
                <tr>
                  <th className="px-3 py-2">#</th>
                  <th className="px-3 py-2">Kind</th>
                  <th className="px-3 py-2">Offset</th>
                  <th className="px-3 py-2">Size</th>
                  <th className="px-3 py-2">Recoverability</th>
                </tr>
              </thead>
              <tbody>
                <PreviewRow kind="JPG" offset="0x000000200000" size="2.41 MiB" pct={98} />
                <PreviewRow kind="PNG" offset="0x000000540000" size="1.18 MiB" pct={94} />
                <PreviewRow kind="MP4" offset="0x0000022a0000" size="48.6 MiB" pct={88} />
                <PreviewRow kind="PDF" offset="0x000004810000" size="312 KiB" pct={91} />
                <PreviewRow kind="DOCX" offset="0x000005220000" size="76 KiB" pct={82} />
              </tbody>
            </table>
          </div>
          <div className="mt-3 flex items-center gap-2 text-xs text-success">
            <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-success" />
            Done. 5 files · 52.4 MiB recoverable · 6.3s
          </div>
        </div>
      </div>
    </div>
  );
}

function PreviewRow({
  kind,
  offset,
  size,
  pct,
}: {
  kind: string;
  offset: string;
  size: string;
  pct: number;
}) {
  return (
    <tr className="border-t border-border">
      <td className="px-3 py-2"><input type="checkbox" defaultChecked readOnly /></td>
      <td className="px-3 py-2 font-medium">{kind}</td>
      <td className="px-3 py-2 font-mono text-muted">{offset}</td>
      <td className="px-3 py-2 font-mono text-muted">{size}</td>
      <td className="px-3 py-2">
        <div className="flex items-center gap-2">
          <div className="h-1.5 w-20 overflow-hidden rounded-full bg-surface2">
            <div className="h-full bg-success" style={{ width: `${pct}%` }} />
          </div>
          <span className="font-mono text-muted">{pct}%</span>
        </div>
      </td>
    </tr>
  );
}

/* ---------- icons (inline SVG, no deps) ---------- */
function DownloadIcon() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="7 10 12 15 17 10" />
      <line x1="12" y1="15" x2="12" y2="3" />
    </svg>
  );
}
function GithubIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M12 .5C5.65.5.5 5.65.5 12c0 5.08 3.29 9.39 7.86 10.91.58.11.79-.25.79-.55v-2.16c-3.2.7-3.87-1.36-3.87-1.36-.52-1.32-1.27-1.67-1.27-1.67-1.04-.71.08-.7.08-.7 1.15.08 1.76 1.18 1.76 1.18 1.02 1.76 2.69 1.25 3.35.96.1-.74.4-1.25.72-1.54-2.55-.29-5.24-1.27-5.24-5.66 0-1.25.45-2.27 1.18-3.07-.12-.29-.51-1.46.11-3.04 0 0 .96-.31 3.15 1.17.91-.25 1.89-.38 2.86-.38.97 0 1.95.13 2.86.38 2.18-1.48 3.14-1.17 3.14-1.17.62 1.58.23 2.75.11 3.04.74.8 1.18 1.82 1.18 3.07 0 4.4-2.7 5.36-5.27 5.65.41.36.78 1.06.78 2.13v3.16c0 .31.21.67.8.55C20.21 21.39 23.5 17.07 23.5 12 23.5 5.65 18.35.5 12 .5z"/>
    </svg>
  );
}
function ShieldIcon() {
  return <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden><path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></svg>;
}
function SearchIcon() {
  return <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden><circle cx="11" cy="11" r="7"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>;
}
function BoltIcon() {
  return <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden><polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/></svg>;
}
function FileIcon() {
  return <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>;
}
function MeterIcon() {
  return <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden><path d="M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0z"/><polyline points="12 7 12 12 15 14"/></svg>;
}
function CodeIcon() {
  return <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden><polyline points="16 18 22 12 16 6"/><polyline points="8 6 2 12 8 18"/></svg>;
}
