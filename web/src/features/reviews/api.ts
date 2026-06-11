import { apiFetch } from '@/lib/http'

export type DailyReview = {
  review_date: string
  review_text: string
  created_at: string
}

export type WeeklyReview = {
  week_start: string
  week_end: string
  review_text: string
  created_at: string
}

export async function listDailyReviews(): Promise<DailyReview[]> {
  const response = await apiFetch('/api/reviews/daily')
  if (!response.ok) {
    throw new Error(`Failed to load daily reviews (${response.status})`)
  }
  const payload = (await response.json()) as { reviews: DailyReview[] }
  return payload.reviews
}

export async function listWeeklyReviews(): Promise<WeeklyReview[]> {
  const response = await apiFetch('/api/reviews/weekly')
  if (!response.ok) {
    throw new Error(`Failed to load weekly reviews (${response.status})`)
  }
  const payload = (await response.json()) as { reviews: WeeklyReview[] }
  return payload.reviews
}
