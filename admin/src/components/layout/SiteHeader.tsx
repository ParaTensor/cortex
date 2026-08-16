import React from 'react'
import { Globe, Moon, Sun, CheckCircle2 } from 'lucide-react'
import { useI18n } from '@/lib/i18n'

export const SiteHeader: React.FC = () => {
  const { locale, setLocale } = useI18n()
  const [isDark, setIsDark] = React.useState(false)

  const toggleTheme = () => {
    setIsDark(!isDark)
    document.documentElement.classList.toggle('dark')
  }

  return (
    <header className="h-14 border-b border-border bg-background px-6 flex items-center justify-between sticky top-0 z-10">
      <div className="flex items-center gap-2">
        <span className="flex h-2 w-2 rounded-full bg-success animate-pulse" />
        <span className="text-xs font-medium text-muted-foreground flex items-center gap-1">
          <CheckCircle2 className="w-3.5 h-3.5 text-success" />
          Gateway Live
        </span>
      </div>

      <div className="flex items-center gap-3">
        {/* Language switcher */}
        <button
          onClick={() => setLocale(locale === 'zh' ? 'en' : 'zh')}
          className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium border border-border rounded-md hover:bg-muted transition-colors cursor-pointer"
          aria-label="Toggle language"
        >
          <Globe className="w-3.5 h-3.5 text-muted-foreground" />
          <span>{locale === 'zh' ? 'English' : '中文'}</span>
        </button>

        {/* Theme toggle */}
        <button
          onClick={toggleTheme}
          className="p-1.5 text-muted-foreground hover:text-foreground border border-border rounded-md hover:bg-muted transition-colors cursor-pointer"
          aria-label="Toggle theme"
        >
          {isDark ? <Sun className="w-4 h-4" /> : <Moon className="w-4 h-4" />}
        </button>
      </div>
    </header>
  )
}
