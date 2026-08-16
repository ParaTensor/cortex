import { create } from 'zustand'
import { zh } from './locales/zh'
import { en } from './locales/en'

type Locale = 'zh' | 'en'

interface I18nStore {
  locale: Locale
  setLocale: (locale: Locale) => void
  t: typeof zh
}

export const useI18n = create<I18nStore>((set) => ({
  locale: 'zh',
  t: zh,
  setLocale: (locale: Locale) =>
    set({
      locale,
      t: locale === 'zh' ? zh : en,
    }),
}))
