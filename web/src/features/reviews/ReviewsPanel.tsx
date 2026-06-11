import { useEffect, useState } from 'react'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { UnauthorizedError } from '@/lib/http'
import {
  listDailyReviews,
  listWeeklyReviews,
  type DailyReview,
  type WeeklyReview,
} from './api'

function formatDate(iso: string): string {
  const date = new Date(`${iso}T00:00:00`)
  if (Number.isNaN(date.getTime())) return iso
  return date.toLocaleDateString(undefined, {
    weekday: 'short',
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  })
}

function ReviewsPanel() {
  const [daily, setDaily] = useState<DailyReview[]>([])
  const [weekly, setWeekly] = useState<WeeklyReview[]>([])
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    async function load() {
      try {
        const [dailyReviews, weeklyReviews] = await Promise.all([
          listDailyReviews(),
          listWeeklyReviews(),
        ])
        setDaily(dailyReviews.reverse())
        setWeekly(weeklyReviews.reverse())
      } catch (err) {
        if (err instanceof UnauthorizedError) return
        setError(err instanceof Error ? err.message : 'Failed to load reviews')
      }
    }
    void load()
  }, [])

  return (
    <div className="flex flex-col gap-8" data-testid="reviews-panel">
      {error && (
        <p
          className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          role="alert"
        >
          {error}
        </p>
      )}

      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-semibold tracking-tight">
          Weekly reviews
        </h2>
        {weekly.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No weekly reviews in the last 30 days.
          </p>
        ) : (
          <ul className="flex flex-col gap-3">
            {weekly.map((review) => (
              <li key={review.week_start}>
                <Card>
                  <CardHeader>
                    <CardTitle className="text-sm">
                      {formatDate(review.week_start)} –{' '}
                      {formatDate(review.week_end)}
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <p className="text-sm whitespace-pre-wrap">
                      {review.review_text}
                    </p>
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="flex flex-col gap-3">
        <h2 className="text-lg font-semibold tracking-tight">Daily reviews</h2>
        {daily.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            No daily reviews in the last 30 days.
          </p>
        ) : (
          <ul className="flex flex-col gap-3">
            {daily.map((review) => (
              <li key={review.review_date}>
                <Card>
                  <CardHeader>
                    <CardTitle className="text-sm">
                      {formatDate(review.review_date)}
                    </CardTitle>
                  </CardHeader>
                  <CardContent>
                    <p className="text-sm whitespace-pre-wrap">
                      {review.review_text}
                    </p>
                  </CardContent>
                </Card>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  )
}

export default ReviewsPanel
